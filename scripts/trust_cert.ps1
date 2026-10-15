# Makes THIS computer trust the "ahsoggg" certificate, so signed Packrat builds show a verified publisher instead
# of "unknown publisher". Only do this if you trust the author: the certificate is added to the current user's
# Trusted Root and Trusted Publishers stores (Windows asks for confirmation for the root store).
#
#   powershell -ExecutionPolicy Bypass -File scripts\trust_cert.ps1 [-Cer certs\ahsoggg.cer]
#
# Compare the printed fingerprint with the one in the README / release notes before agreeing.
param([string]$Cer = (Join-Path (Split-Path -Parent $PSScriptRoot) "certs\ahsoggg.cer"))

$ErrorActionPreference = "Stop"
$c = New-Object System.Security.Cryptography.X509Certificates.X509Certificate2 (Resolve-Path $Cer)
Write-Host "Certificate:         $($c.Subject)"
Write-Host "SHA-256 fingerprint: $((Get-FileHash $Cer -Algorithm SHA256).Hash)"
Import-Certificate -FilePath $Cer -CertStoreLocation Cert:\CurrentUser\Root | Out-Null
Import-Certificate -FilePath $Cer -CertStoreLocation Cert:\CurrentUser\TrustedPublisher | Out-Null
Write-Host "Done. Open the file's Properties > Digital Signatures to see it as valid."
