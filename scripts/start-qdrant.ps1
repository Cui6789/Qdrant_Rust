# Starts a local Qdrant binary (v1.20.2, Windows x86_64) for development and
# integration tests. Idempotent: exits 0 immediately when port 6334 is already
# accepting TCP connections.
$ErrorActionPreference = "Stop"

$Port = 6334
$RepoRoot = Split-Path -Parent $PSScriptRoot
$ToolsDir = Join-Path $RepoRoot ".tools"
$ZipPath = Join-Path $ToolsDir "qdrant.zip"
$QdrantDir = Join-Path $ToolsDir "qdrant"
$QdrantExe = Join-Path $QdrantDir "qdrant.exe"
$DownloadUrl = "https://github.com/qdrant/qdrant/releases/download/v1.20.2/qdrant-x86_64-pc-windows-msvc.zip"

function Test-QdrantPort {
    param([int]$TargetPort)
    try {
        $client = New-Object System.Net.Sockets.TcpClient
        $async = $client.BeginConnect("127.0.0.1", $TargetPort, $null, $null)
        if (-not $async.AsyncWaitHandle.WaitOne(500)) {
            $client.Close()
            return $false
        }
        $client.EndConnect($async)
        $client.Close()
        return $true
    } catch {
        return $false
    }
}

if (Test-QdrantPort -TargetPort $Port) {
    Write-Host "Qdrant is already listening on port $Port"
    exit 0
}

New-Item -ItemType Directory -Force -Path $ToolsDir | Out-Null

if (-not (Test-Path $QdrantExe)) {
    if (-not (Test-Path $ZipPath)) {
        Write-Host "Downloading Qdrant v1.20.2 from $DownloadUrl"
        Invoke-WebRequest -Uri $DownloadUrl -OutFile $ZipPath
    }
    Write-Host "Extracting $ZipPath into $QdrantDir"
    if (Test-Path $QdrantDir) {
        Remove-Item -Recurse -Force $QdrantDir
    }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    # Expand-Archive -Force uses an overload that Windows PowerShell 5.1 on
    # this machine does not expose; the two-argument ZipFile call is portable.
    [System.IO.Compression.ZipFile]::ExtractToDirectory($ZipPath, $QdrantDir)
}

# The archive may place qdrant.exe at the root or inside a nested directory.
if (-not (Test-Path $QdrantExe)) {
    $found = Get-ChildItem -Path $QdrantDir -Filter "qdrant.exe" -Recurse |
        Select-Object -First 1
    if ($null -eq $found) {
        throw "qdrant.exe was not found after extracting $ZipPath"
    }
    $QdrantExe = $found.FullName
    $QdrantDir = $found.DirectoryName
}

Write-Host "Starting $QdrantExe (working directory: $QdrantDir)"
Start-Process -FilePath $QdrantExe -WorkingDirectory $QdrantDir

$deadline = (Get-Date).AddSeconds(30)
while ((Get-Date) -lt $deadline) {
    if (Test-QdrantPort -TargetPort $Port) {
        Write-Host "Qdrant is listening on port $Port"
        exit 0
    }
    Start-Sleep -Milliseconds 500
}

throw "Qdrant did not start listening on port $Port within 30 seconds"