[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('Prepare', 'Finalize')]
    [string]$Mode,

    [Parameter(Mandatory = $true)]
    [string]$InputRoot,

    [Parameter(Mandatory = $true)]
    [string]$SigningRoot,

    [Parameter(Mandatory = $true)]
    [string]$OutputRoot,

    [Parameter(Mandatory = $true)]
    [string]$SourceSha,

    [Parameter(Mandatory = $true)]
    [string]$Version,

    [Parameter(Mandatory = $true)]
    [long]$RunId,

    [Parameter(Mandatory = $true)]
    [int]$RunAttempt,

    [string]$ExpectedOrganization = 'PARTNERNET SOFTWARE PTY LTD',

    [string]$ArtifactManifest = 'scripts/artifacts.json',

    [string]$ReleasePolicy = 'release-policy.json',

    [string]$StatePath = 'windows-signing-state.json'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Get-Sha256([string]$Path) {
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Write-Json([object]$Value, [string]$Path) {
    $Value | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $Path -Encoding utf8NoBOM
}

function Require([bool]$Condition, [string]$Message) {
    if (-not $Condition) {
        throw $Message
    }
}

function Test-ProductVersion([string]$Actual) {
    return $Actual -match ('^' + [regex]::Escape($Version) + '(?:\.0)?$')
}

$inputPath = [IO.Path]::GetFullPath($InputRoot)
$signingPath = [IO.Path]::GetFullPath($SigningRoot)
$outputPath = [IO.Path]::GetFullPath($OutputRoot)
$stateFile = [IO.Path]::GetFullPath($StatePath)
$receiptName = 'windows-signing-receipt.json'
$manifest = Get-Content -LiteralPath $ArtifactManifest -Raw | ConvertFrom-Json
$policy = Get-Content -LiteralPath $ReleasePolicy -Raw | ConvertFrom-Json

Require ($SourceSha -match '^[0-9a-f]{40}$') 'source SHA must be lowercase 40-hex'
Require ($policy.version -eq $Version) 'release policy version mismatch'
Require ($policy.signing.windows -eq 'required') 'Prepare/Finalize require signing.windows=required'
Require ($manifest.schema_version -eq 2) 'unsupported artifacts manifest schema'

$platforms = @(
    [ordered]@{ id = 'windows-x86_64'; arch = 'x86_64' },
    [ordered]@{ id = 'windows-aarch64'; arch = 'aarch64' }
)
$noticeFiles = @('LICENSE-APACHE', 'LICENSE-MIT', 'THIRD_PARTY_NOTICES.md', 'artifacts.json')

if ($Mode -eq 'Prepare') {
    foreach ($owned in @($signingPath, $outputPath)) {
        if (Test-Path -LiteralPath $owned) {
            Remove-Item -LiteralPath $owned -Recurse -Force
        }
        New-Item -ItemType Directory -Path $owned -Force | Out-Null
    }

    $statePlatforms = [ordered]@{}
    $catalog = [System.Collections.Generic.List[string]]::new()
    foreach ($platform in $platforms) {
        $spec = @($manifest.platforms | Where-Object {
            $_.os -eq 'windows' -and $_.arch -eq $platform.arch
        })
        Require ($spec.Count -eq 1) "manifest platform count mismatch: $($platform.id)"
        $peNames = @($spec[0].executables.name) + @($spec[0].libraries.name)
        Require ($peNames.Count -eq 5) "expected five PE inputs: $($platform.id)"
        Require ((@($peNames | Sort-Object -Unique)).Count -eq 5) "duplicate PE input: $($platform.id)"

        $part = Join-Path $inputPath $platform.id
        Require (Test-Path -LiteralPath $part -PathType Container) "missing unsigned part: $($platform.id)"
        $finalPart = Join-Path $outputPath $platform.id
        Copy-Item -LiteralPath $part -Destination $finalPart -Recurse

        $archiveName = "agenterm-$Version-windows-$($platform.arch).zip"
        $archive = Join-Path $part $archiveName
        $checksum = "$archive.sha256"
        $provenance = "$archive.provenance.json"
        foreach ($required in @($archive, $checksum, $provenance)) {
            Require (Test-Path -LiteralPath $required -PathType Leaf) "missing unsigned companion: $required"
        }
        $archiveHash = Get-Sha256 $archive
        $checksumText = (Get-Content -LiteralPath $checksum -Raw).Trim()
        Require ($checksumText -eq "$archiveHash  $archiveName") "unsigned checksum mismatch: $archiveName"
        $provenanceValue = Get-Content -LiteralPath $provenance -Raw | ConvertFrom-Json
        Require ($provenanceValue.source_commit -eq $SourceSha) "unsigned source mismatch: $archiveName"
        Require ($provenanceValue.version -eq $Version) "unsigned version mismatch: $archiveName"
        Require ($provenanceValue.os -eq 'windows' -and $provenanceValue.arch -eq $platform.arch) "unsigned platform mismatch: $archiveName"
        Require ($provenanceValue.sha256 -eq $archiveHash) "unsigned provenance hash mismatch: $archiveName"
        Require ($provenanceValue.signed -eq $false -and $provenanceValue.notarized -eq $false) "unsigned provenance state mismatch: $archiveName"

        $payload = Join-Path $signingPath $platform.id
        New-Item -ItemType Directory -Path $payload -Force | Out-Null
        Expand-Archive -LiteralPath $archive -DestinationPath $payload
        $actual = @(Get-ChildItem -LiteralPath $payload -File -Recurse | ForEach-Object {
            [IO.Path]::GetRelativePath($payload, $_.FullName).Replace('\', '/')
        } | Sort-Object)
        $expected = @($peNames + $noticeFiles | Sort-Object)
        Require (($actual -join "`n") -eq ($expected -join "`n")) "archive payload set mismatch: $($platform.id)"

        $assets = [ordered]@{}
        foreach ($name in $peNames) {
            $file = Join-Path $payload $name
            $signature = Get-AuthenticodeSignature -LiteralPath $file
            Require ($signature.Status -eq [System.Management.Automation.SignatureStatus]::NotSigned) "input is already signed: $($platform.id)/$name"
            $versionInfo = (Get-Item -LiteralPath $file).VersionInfo
            Require ($versionInfo.ProductName -eq 'AgenTerm') "ProductName mismatch: $($platform.id)/$name"
            Require (Test-ProductVersion $versionInfo.ProductVersion) "ProductVersion mismatch: $($platform.id)/$name"
            $relative = "$($platform.id)/$name"
            $catalog.Add($relative.Replace('/', '\'))
            $assets[$name] = [ordered]@{
                path = $relative
                before_sha256 = Get-Sha256 $file
                before_bytes = (Get-Item -LiteralPath $file).Length
                product_name = $versionInfo.ProductName
                product_version = $versionInfo.ProductVersion
            }
        }
        $statePlatforms[$platform.id] = [ordered]@{
            arch = $platform.arch
            archive_name = $archiveName
            archive_before_sha256 = $archiveHash
            assets = $assets
        }
    }
    $catalog | Sort-Object | Set-Content -LiteralPath (Join-Path $signingPath 'signing-catalog.txt') -Encoding ascii
    Write-Json ([ordered]@{
        schema_version = 1
        kind = 'agenterm-windows-signing-state'
        source_sha = $SourceSha
        version = $Version
        run = [ordered]@{ id = $RunId; attempt = $RunAttempt }
        platforms = $statePlatforms
    }) $stateFile
    return
}

Require (Test-Path -LiteralPath $stateFile -PathType Leaf) 'missing signing state'
$state = Get-Content -LiteralPath $stateFile -Raw | ConvertFrom-Json
Require ($state.schema_version -eq 1 -and $state.kind -eq 'agenterm-windows-signing-state') 'signing state schema mismatch'
Require ($state.source_sha -eq $SourceSha -and $state.version -eq $Version) 'signing state source mismatch'
Require ($state.run.id -eq $RunId -and $state.run.attempt -eq $RunAttempt) 'signing state run mismatch'

$receiptPlatforms = [ordered]@{}
$receiptAssets = [ordered]@{}
foreach ($platform in $platforms) {
    $statePlatform = $state.platforms.$($platform.id)
    Require ($null -ne $statePlatform) "missing state platform: $($platform.id)"
    $spec = @($manifest.platforms | Where-Object {
        $_.os -eq 'windows' -and $_.arch -eq $platform.arch
    })[0]
    $peNames = @($spec.executables.name) + @($spec.libraries.name)
    $payload = Join-Path $signingPath $platform.id
    $actual = @(Get-ChildItem -LiteralPath $payload -File -Recurse | ForEach-Object {
        [IO.Path]::GetRelativePath($payload, $_.FullName).Replace('\', '/')
    } | Sort-Object)
    $expected = @($peNames + $noticeFiles | Sort-Object)
    Require (($actual -join "`n") -eq ($expected -join "`n")) "signed payload set mismatch: $($platform.id)"

    foreach ($name in $peNames) {
        $file = Join-Path $payload $name
        $signature = Get-AuthenticodeSignature -LiteralPath $file
        Require ($signature.Status -eq [System.Management.Automation.SignatureStatus]::Valid) "invalid Authenticode: $($platform.id)/$name"
        Require ($null -ne $signature.SignerCertificate -and $null -ne $signature.TimeStamperCertificate) "missing signer or timestamp: $($platform.id)/$name"
        $publisherPattern = '(?:^|,\s*)O=' + [regex]::Escape($ExpectedOrganization) + '(?:,|$)'
        Require ($signature.SignerCertificate.Subject -match $publisherPattern) "publisher mismatch: $($platform.id)/$name"
        $versionInfo = (Get-Item -LiteralPath $file).VersionInfo
        Require ($versionInfo.ProductName -eq 'AgenTerm') "signed ProductName mismatch: $($platform.id)/$name"
        Require (Test-ProductVersion $versionInfo.ProductVersion) "signed ProductVersion mismatch: $($platform.id)/$name"
        $before = $statePlatform.assets.$name
        $afterHash = Get-Sha256 $file
        Require ($afterHash -ne $before.before_sha256) "signing did not change bytes: $($platform.id)/$name"
        $receiptAssets["$($platform.id)/$name"] = [ordered]@{
            path = "$($platform.id)/$name"
            before_sha256 = $before.before_sha256
            after_sha256 = $afterHash
            after_bytes = (Get-Item -LiteralPath $file).Length
            authenticode_status = "$($signature.Status)"
            product_name = $versionInfo.ProductName
            product_version = $versionInfo.ProductVersion
            signer_subject = $signature.SignerCertificate.Subject
            signer_issuer = $signature.SignerCertificate.Issuer
            signer_thumbprint = $signature.SignerCertificate.Thumbprint
            signer_not_before = $signature.SignerCertificate.NotBefore.ToUniversalTime().ToString('o')
            signer_not_after = $signature.SignerCertificate.NotAfter.ToUniversalTime().ToString('o')
            timestamp_subject = $signature.TimeStamperCertificate.Subject
            timestamp_issuer = $signature.TimeStamperCertificate.Issuer
        }
    }

    $finalPart = Join-Path $outputPath $platform.id
    $archive = Join-Path $finalPart $statePlatform.archive_name
    # libarchive selects ZIP from the final extension; keep `.zip` on the
    # temporary file and publish only after creation succeeds.
    $temporaryArchive = "$archive.signing.tmp.zip"
    if (Test-Path -LiteralPath $temporaryArchive) {
        Remove-Item -LiteralPath $temporaryArchive -Force
    }
    & tar -a -c -f $temporaryArchive -C $payload .
    Require ($LASTEXITCODE -eq 0 -and (Test-Path -LiteralPath $temporaryArchive -PathType Leaf)) "signed archive creation failed: $($platform.id)"
    Remove-Item -LiteralPath $archive -Force
    Move-Item -LiteralPath $temporaryArchive -Destination $archive
    $archiveHash = Get-Sha256 $archive
    $archiveBytes = (Get-Item -LiteralPath $archive).Length
    $payloadBytes = (Get-ChildItem -LiteralPath $payload -File -Recurse | Measure-Object -Property Length -Sum).Sum
    $provenancePath = "$archive.provenance.json"
    $provenance = Get-Content -LiteralPath $provenancePath -Raw | ConvertFrom-Json
    $provenance.signed = $true
    $provenance.sha256 = $archiveHash
    $provenance.archive_bytes = $archiveBytes
    $provenance.payload_uncompressed_bytes = $payloadBytes
    Write-Json $provenance $provenancePath
    "$archiveHash  $($statePlatform.archive_name)" | Set-Content -LiteralPath "$archive.sha256" -Encoding ascii
    $receiptPlatforms[$platform.id] = [ordered]@{
        arch = $platform.arch
        archive_name = $statePlatform.archive_name
        before_sha256 = $statePlatform.archive_before_sha256
        after_sha256 = $archiveHash
        after_bytes = $archiveBytes
        payload_uncompressed_bytes = $payloadBytes
    }
}

$receipt = [ordered]@{
    schema_version = 1
    kind = 'agenterm-azure-artifact-signing'
    source_sha = $SourceSha
    version = $Version
    signing_provider = 'azure-artifact-signing'
    publisher_organization = $ExpectedOrganization
    file_digest = 'SHA256'
    timestamp_rfc3161 = 'http://timestamp.acs.microsoft.com'
    timestamp_digest = 'SHA256'
    release_eligible = $true
    platform_count = 2
    asset_count = 10
    run = [ordered]@{ id = $RunId; attempt = $RunAttempt }
    platforms = $receiptPlatforms
    assets = $receiptAssets
}
Write-Json $receipt (Join-Path (Join-Path $outputPath 'windows-x86_64') $receiptName)
