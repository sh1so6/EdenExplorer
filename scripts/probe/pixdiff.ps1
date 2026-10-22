# Compare two PNGs pixel by pixel; prints the differing pixel count and bounding box.
param([Parameter(Mandatory)][string]$A, [Parameter(Mandatory)][string]$B)
Add-Type -AssemblyName System.Drawing
$bmA = [System.Drawing.Bitmap]::new((Resolve-Path $A).Path)
$bmB = [System.Drawing.Bitmap]::new((Resolve-Path $B).Path)
if ($bmA.Width -ne $bmB.Width -or $bmA.Height -ne $bmB.Height) { "size differs"; exit 1 }
$n = 0; $minX = [int]::MaxValue; $minY = [int]::MaxValue; $maxX = -1; $maxY = -1
for ($y = 0; $y -lt $bmA.Height; $y++) {
  for ($x = 0; $x -lt $bmA.Width; $x++) {
    if ($bmA.GetPixel($x, $y).ToArgb() -ne $bmB.GetPixel($x, $y).ToArgb()) {
      $n++
      if ($x -lt $minX) { $minX = $x }; if ($x -gt $maxX) { $maxX = $x }
      if ($y -lt $minY) { $minY = $y }; if ($y -gt $maxY) { $maxY = $y }
    }
  }
}
$bmA.Dispose(); $bmB.Dispose()
if ($n -eq 0) { "identical" } else { "$n px differ, bbox x=$minX..$maxX y=$minY..$maxY" }
