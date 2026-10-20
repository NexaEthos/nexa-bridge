# Wiki: docs/wiki/release.md
param([Parameter(Mandatory)][string]$Path)
$ErrorActionPreference = 'Stop'

foreach ($name in 'AZURE_SIGNING_ENDPOINT', 'AZURE_SIGNING_ACCOUNT', 'AZURE_SIGNING_PROFILE') {
  if (-not [Environment]::GetEnvironmentVariable($name)) { throw "$name is missing from the signing step" }
}
$params = @{
  Endpoint                            = $env:AZURE_SIGNING_ENDPOINT
  CodeSigningAccountName              = $env:AZURE_SIGNING_ACCOUNT
  CertificateProfileName              = $env:AZURE_SIGNING_PROFILE
  Files                               = $Path
  FileDigest                          = 'SHA256'
  TimestampRfc3161                    = 'http://timestamp.acs.microsoft.com'
  TimestampDigest                     = 'SHA256'
  ExcludeEnvironmentCredential        = $true
  ExcludeWorkloadIdentityCredential   = $true
  ExcludeManagedIdentityCredential    = $true
  ExcludeSharedTokenCacheCredential   = $true
  ExcludeVisualStudioCredential       = $true
  ExcludeVisualStudioCodeCredential   = $true
  ExcludeAzurePowerShellCredential    = $true
  ExcludeAzureDeveloperCliCredential  = $true
  ExcludeInteractiveBrowserCredential = $true
}
Invoke-ArtifactSigning @params
$signature = Get-AuthenticodeSignature $Path
if ($signature.Status -ne 'Valid') { throw "${Path}: signature $($signature.Status) after signing" }
