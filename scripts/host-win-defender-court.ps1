param(
  [Parameter(Mandatory)] [string] $CandidateDir,
  [Parameter(Mandatory)] [string] $ReceiptPath,
  [string] $Court = 'windows-host'
)
# Host-native counterpart of scripts/utm-win-defender-court.sh for an operator
# already on a real Windows machine. It downloads nothing: CandidateDir is an
# extracted `release-candidate-<run>` artifact (manifest at its root, archives
# under payload/). The MpCmdRun invocation and asset rows match the UTM court,
# and the receipt it writes is the `agenterm-defender-court` that
# scripts/agenterm-reputation-court.py qualify consumes. The verdict is machine
# derived; there is no override.
$ErrorActionPreference = 'Stop'
if ($env:AGENTERM_DEFENDER_VERDICT) {
  throw 'the Defender verdict is machine-derived; remove AGENTERM_DEFENDER_VERDICT'
}
$manifestFile = Get-ChildItem -LiteralPath $CandidateDir -Filter 'agenterm-*-candidate-manifest.json' -File
if (@($manifestFile).Count -ne 1) { throw 'expected exactly one Candidate manifest' }
$manifest = Get-Content -LiteralPath $manifestFile.FullName -Raw | ConvertFrom-Json
$payload = Join-Path $CandidateDir 'payload'

$mp = Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Windows Defender\MpCmdRun.exe'
if (-not (Test-Path -LiteralPath $mp)) { throw 'MpCmdRun.exe was not found' }
$status = Get-MpComputerStatus
if (-not $status.AMServiceEnabled -or -not $status.AntivirusEnabled) {
  throw 'Microsoft Defender antivirus is not enabled on this host'
}
& $mp -SignatureUpdate | Out-Null

# Negative control: a clean verdict means nothing unless this scanner reports
# the EICAR test string through the same invocation.
$controlDir = Join-Path $env:TEMP ('agenterm-defender-control-' + [guid]::NewGuid())
New-Item -ItemType Directory -Path $controlDir | Out-Null
try {
  $control = Join-Path $controlDir 'control.txt'
  [IO.File]::WriteAllText($control, 'X5O!P%@AP[4\PZX54(P^)7CC)7}$EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*')
  & $mp -Scan -ScanType 3 -File $control -DisableRemediation | Out-Null
  $controlCode = $LASTEXITCODE
} finally {
  Remove-Item -LiteralPath $controlDir -Recurse -Force -ErrorAction SilentlyContinue
}
if ($controlCode -ne 2) { throw "negative control: EICAR scan returned $controlCode, expected 2" }

$status = Get-MpComputerStatus
$rows = @()
foreach ($asset in @($manifest.assets | Where-Object { $_.os -eq 'windows' })) {
  $platformId = "windows-$($asset.arch)"
  $file = Join-Path $payload $asset.name
  $before = (Get-FileHash -Algorithm SHA256 -LiteralPath $file).Hash.ToLowerInvariant()
  if ($before -ne $asset.sha256) { throw "sealed archive digest drift for $platformId" }
  # MpCmdRun exit 0 = no threat, 2 = threat found, anything else = scan error.
  $output = & $mp -Scan -ScanType 3 -File $file -DisableRemediation 2>&1 | Out-String
  $code = $LASTEXITCODE
  $after = (Get-FileHash -Algorithm SHA256 -LiteralPath $file).Hash.ToLowerInvariant()
  if ($code -ne 0) { throw "Defender scan of $platformId returned $code`n$output" }
  if ($after -ne $before) { throw "Defender altered the bytes of $platformId" }
  $rows += [ordered]@{
    platform_id      = $platformId
    post_scan_sha256 = $after
    scan_exit_code   = $code
    sha256           = $before
    threats_detected = 0
  }
}
if ($rows.Count -ne 2) { throw "expected two Windows archives, scanned $($rows.Count)" }

$receipt = [ordered]@{
  assets           = @($rows | Sort-Object { $_.platform_id })
  candidate_run    = [ordered]@{ attempt = [int]$manifest.run.attempt; id = [long]$manifest.run.id }
  completed_at     = [DateTime]::UtcNow.ToString('o')
  court            = $Court
  kind             = 'agenterm-defender-court'
  negative_control = [ordered]@{ eicar_exit_code = $controlCode }
  scan_path        = 'host-mpcmdrun'
  scanner          = [ordered]@{
    engine_version    = $status.AMEngineVersion
    product_version   = $status.AMProductVersion
    signature_version = $status.AntivirusSignatureVersion
  }
  schema_version   = 1
  source_sha       = $manifest.source_sha
  verdict          = 'clean'
  version          = $manifest.version
}
[IO.File]::WriteAllText($ReceiptPath, ($receipt | ConvertTo-Json -Depth 6) + "`n", (New-Object Text.UTF8Encoding $false))
"CLEAN agenterm Defender receipt assets=$($rows.Count) court=$Court control=$controlCode"
