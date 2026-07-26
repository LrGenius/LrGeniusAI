# Downloads InsightFace's buffalo_l face-recognition models if they aren't
# already present. Same location Python's insightface library uses
# (%USERPROFILE%\.insightface), so a prior Python InsightFace install is
# detected and reused instead of re-downloading.

$ErrorActionPreference = "Stop"

$targetDir = Join-Path $env:USERPROFILE ".insightface\models\buffalo_l"
$detFile = Join-Path $targetDir "det_10g.onnx"
$recFile = Join-Path $targetDir "w600k_r50.onnx"

if ((Test-Path $detFile) -and (Test-Path $recFile)) {
    exit 0
}

New-Item -ItemType Directory -Force -Path $targetDir | Out-Null

$zipUrl = "https://github.com/deepinsight/insightface/releases/download/v0.7/buffalo_l.zip"
$tmpZip = Join-Path $env:TEMP "buffalo_l_$([guid]::NewGuid().ToString('N')).zip"
$tmpExtract = Join-Path $env:TEMP "buffalo_l_extract_$([guid]::NewGuid().ToString('N'))"

try {
    Invoke-WebRequest -Uri $zipUrl -OutFile $tmpZip -UseBasicParsing
    Expand-Archive -Path $tmpZip -DestinationPath $tmpExtract -Force

    $detSrc = Get-ChildItem -Path $tmpExtract -Filter "det_10g.onnx" -Recurse | Select-Object -First 1
    $recSrc = Get-ChildItem -Path $tmpExtract -Filter "w600k_r50.onnx" -Recurse | Select-Object -First 1

    if ($detSrc -and $recSrc) {
        Copy-Item $detSrc.FullName $detFile -Force
        Copy-Item $recSrc.FullName $recFile -Force
    } else {
        Write-Warning "Expected model files not found in buffalo_l.zip"
    }
} catch {
    Write-Warning "Failed to download InsightFace models; face detection will be unavailable: $_"
} finally {
    Remove-Item -Force -ErrorAction SilentlyContinue $tmpZip
    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $tmpExtract
}
