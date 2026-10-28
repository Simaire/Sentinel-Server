# Construit et publie l'image du serveur (x64 et ARM64) sur le registre GitHub :
#   ghcr.io/simaire/sentinel-server:<version de Cargo.toml> et :latest
#
#   .\publish-image.ps1              (une fois connecté : docker login ghcr.io -u Simaire)
#   .\publish-image.ps1 -NoLatest    (sans déplacer « latest »)
#
# Compilation croisée sans émulation (voir le Dockerfile) : quelques minutes, même
# depuis un PC ARM64.

param([switch]$NoLatest)

$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

$image = 'ghcr.io/simaire/sentinel-server'
$version = (Select-String -Path Cargo.toml -Pattern '^version = "(.+)"' | Select-Object -First 1).Matches.Groups[1].Value
if (-not $version) { throw 'Version introuvable dans Cargo.toml.' }

$tags = @('-t', "${image}:$version")
if (-not $NoLatest) { $tags += @('-t', "${image}:latest") }

Write-Host "Publication de ${image}:$version (linux/amd64 + linux/arm64)…"
docker buildx build --platform linux/amd64,linux/arm64 @tags --push .
if ($LASTEXITCODE -ne 0) { throw 'La construction a échoué (voir ci-dessus).' }

Write-Host ""
Write-Host "Publié : ${image}:$version"
Write-Host "Sur le serveur : docker compose pull && docker compose up -d"
