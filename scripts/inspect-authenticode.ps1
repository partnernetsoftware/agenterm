[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Path,

    [string]$ExpectedOrganization = 'PARTNERNET SOFTWARE PTY LTD',

    [string]$ExpectedProductName = '',

    [string]$ExpectedProductVersion = ''
)

$ErrorActionPreference = 'Stop'
if ($null -eq (Get-Command Get-AuthenticodeSignature -ErrorAction SilentlyContinue)) {
    [Console]::Error.WriteLine(
        'Get-AuthenticodeSignature is unavailable; run this court on Windows.'
    )
    exit 69
}
$resolved = (Resolve-Path -LiteralPath $Path).Path
$signature = Get-AuthenticodeSignature -LiteralPath $resolved
$signer = $signature.SignerCertificate
$timestamp = $signature.TimeStamperCertificate
$item = Get-Item -LiteralPath $resolved
$versionInfo = $item.VersionInfo
$sha256 = (Get-FileHash -LiteralPath $resolved -Algorithm SHA256).Hash.ToLowerInvariant()

$result = [ordered]@{
    schema_version = 2
    file_name = $item.Name
    sha256 = $sha256
    size_bytes = [int64]$item.Length
    status = [string]$signature.Status
    status_message = $signature.StatusMessage
    product_name = $versionInfo.ProductName
    product_version = $versionInfo.ProductVersion
    file_description = $versionInfo.FileDescription
    original_filename = $versionInfo.OriginalFilename
    signer = if ($null -eq $signer) { $null } else { [ordered]@{
        subject = $signer.Subject
        issuer = $signer.Issuer
        thumbprint = $signer.Thumbprint
        not_before = $signer.NotBefore.ToUniversalTime().ToString('o')
        not_after = $signer.NotAfter.ToUniversalTime().ToString('o')
    } }
    timestamp_certificate = if ($null -eq $timestamp) { $null } else { [ordered]@{
        subject = $timestamp.Subject
        issuer = $timestamp.Issuer
        thumbprint = $timestamp.Thumbprint
    } }
}

$result | ConvertTo-Json -Depth 5

if ($signature.Status -eq [System.Management.Automation.SignatureStatus]::NotSigned) {
    exit 2
}
if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid) {
    exit 3
}
if ($null -eq $signer) {
    exit 3
}
$publisherPattern = '(?:^|,\s*)O=' + [regex]::Escape($ExpectedOrganization) + '(?:,|$)'
if ($signer.Subject -notmatch $publisherPattern) {
    exit 4
}
if ($null -eq $timestamp) {
    exit 5
}
if (($ExpectedProductName -ne '' -and $versionInfo.ProductName -ne $ExpectedProductName) -or
    ($ExpectedProductVersion -ne '' -and $versionInfo.ProductVersion -notmatch ('^' + [regex]::Escape($ExpectedProductVersion) + '(?:\.0)?$'))) {
    exit 6
}
exit 0
