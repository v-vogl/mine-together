# Deploy Mine Together to the server: build the client here, upload it with the server source, build
# the server there and restart it. Needs SSH key login to root on the server.
#
#   .\deploy\deploy.ps1 -Server 1.2.3.4             update the game
#   .\deploy\deploy.ps1 -Server 1.2.3.4 -Setup example.com   first time: set up the server for that domain
param(
    [Parameter(Mandatory)][string]$Server,
    [string]$Setup
)
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot

Push-Location "$root\client"
npm run build
if ($LASTEXITCODE) { throw "client build failed" }
Pop-Location

$archive = Join-Path $env:TEMP "mine-together-deploy.tar.gz"
tar -czf $archive -C $root server/Cargo.toml server/Cargo.lock server/src client/dist deploy
if ($LASTEXITCODE) { throw "packing failed" }
scp $archive "root@${Server}:/tmp/mine-together.tar.gz"
if ($LASTEXITCODE) { throw "upload failed" }

$unpack = "rm -rf /tmp/mt && mkdir /tmp/mt && tar -xzf /tmp/mine-together.tar.gz -C /tmp/mt"
if ($Setup) { ssh "root@$Server" "$unpack && bash /tmp/mt/deploy/setup.sh $Setup" }
if ($LASTEXITCODE) { throw "setup failed" }
ssh "root@$Server" "$unpack && bash /tmp/mt/deploy/remote-deploy.sh"
if ($LASTEXITCODE) { throw "deploy failed" }
Write-Host "Deployed."
