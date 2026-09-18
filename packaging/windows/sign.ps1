# Authenticode-signs one or more files with Azure Artifact Signing.
#
# The release workflow calls this directly for the built binary and hands it
# to Inno Setup as its SignTool, so the installer, its embedded uninstaller
# and the program itself all carry the same signature and timestamp.
#
# Expects the `sign` CLI (dotnet/sign) on PATH or in SIGN_TOOL_DIR, and these
# environment variables: AZURE_TENANT_ID, AZURE_CLIENT_ID, AZURE_CLIENT_SECRET
# (service principal, picked up by DefaultAzureCredential), plus
# AZURE_SIGNING_ENDPOINT, AZURE_SIGNING_ACCOUNT and AZURE_SIGNING_PROFILE.
#
#   pwsh -File packaging\windows\sign.ps1 path\to\file.exe [more files...]

param(
    [Parameter(Mandatory = $true, ValueFromRemainingArguments = $true)]
    [string[]] $Files
)

$ErrorActionPreference = 'Stop'

foreach ($name in 'AZURE_TENANT_ID', 'AZURE_CLIENT_ID', 'AZURE_CLIENT_SECRET',
                  'AZURE_SIGNING_ENDPOINT', 'AZURE_SIGNING_ACCOUNT', 'AZURE_SIGNING_PROFILE') {
    if (-not [Environment]::GetEnvironmentVariable($name)) {
        throw "sign.ps1: $name is not set"
    }
}

$sign = if ($env:SIGN_TOOL_DIR) { Join-Path $env:SIGN_TOOL_DIR 'sign.exe' } else { 'sign' }

foreach ($file in $Files) {
    if (-not (Test-Path $file)) { throw "sign.ps1: $file does not exist" }
    & $sign code artifact-signing `
        --artifact-signing-endpoint $env:AZURE_SIGNING_ENDPOINT `
        --artifact-signing-account $env:AZURE_SIGNING_ACCOUNT `
        --artifact-signing-certificate-profile $env:AZURE_SIGNING_PROFILE `
        --file-digest SHA256 `
        --timestamp-url http://timestamp.acs.microsoft.com `
        --timestamp-digest SHA256 `
        --verbosity Information `
        $file
    if ($LASTEXITCODE -ne 0) { throw "sign.ps1: signing $file failed with exit code $LASTEXITCODE" }

    $signature = Get-AuthenticodeSignature $file
    if ($signature.Status -ne 'Valid') {
        throw "sign.ps1: $file signature status after signing: $($signature.Status)"
    }
    Write-Host "signed $file as $($signature.SignerCertificate.Subject)"
}
