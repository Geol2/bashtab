param([string]$outDir)
# Draws the bashtab icon (dark rounded window, orange active tab, ">_" prompt)
# at each size natively and packs them into a PNG-compressed .ico.
Add-Type -AssemblyName System.Drawing

function RoundRect([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
  $p = New-Object Drawing.Drawing2D.GraphicsPath
  $d = $r * 2
  $p.AddArc($x, $y, $d, $d, 180, 90)
  $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
  $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
  $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
  $p.CloseFigure()
  $p
}

function TopRoundRect([float]$x, [float]$y, [float]$w, [float]$h, [float]$r) {
  $p = New-Object Drawing.Drawing2D.GraphicsPath
  $d = $r * 2
  $p.AddArc($x, $y, $d, $d, 180, 90)
  $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
  $p.AddLine($x + $w, $y + $h, $x, $y + $h)
  $p.CloseFigure()
  $p
}

function Draw([int]$size) {
  $bmp = New-Object Drawing.Bitmap $size, $size, ([Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $g = [Drawing.Graphics]::FromImage($bmp)
  $g.SmoothingMode = 'AntiAlias'
  $g.PixelOffsetMode = 'HighQuality'
  $g.Clear([Drawing.Color]::Transparent)
  $s = $size / 256.0

  $bg = [Drawing.Color]::FromArgb(255, 27, 27, 31)
  $bar = [Drawing.Color]::FromArgb(255, 44, 44, 50)
  $orange = [Drawing.Color]::FromArgb(255, 241, 78, 50)
  $gray = [Drawing.Color]::FromArgb(255, 80, 80, 90)
  $fg = [Drawing.Color]::FromArgb(255, 236, 236, 241)

  # window
  $m = 12 * $s
  $win = RoundRect $m $m ($size - 2 * $m) ($size - 2 * $m) (44 * $s)
  $g.FillPath((New-Object Drawing.SolidBrush $bg), $win)

  # tab strip: active (orange) + inactive (gray) tab
  $g.SetClip($win)
  $g.FillRectangle((New-Object Drawing.SolidBrush $bar), $m, $m, $size, 64 * $s)
  $tabY = $m + 18 * $s; $tabH = 46 * $s
  $g.FillPath((New-Object Drawing.SolidBrush $orange), (TopRoundRect ($m + 26 * $s) $tabY (92 * $s) $tabH (14 * $s)))
  if ($size -ge 32) {
    $g.FillPath((New-Object Drawing.SolidBrush $gray), (TopRoundRect ($m + 128 * $s) $tabY (70 * $s) $tabH (14 * $s)))
  }
  $g.ResetClip()
  # outline so the dark window stays visible on a dark taskbar
  $edge = New-Object Drawing.Pen ([Drawing.Color]::FromArgb(255, 78, 78, 88)), ([Math]::Max(1, 5 * $s))
  $g.DrawPath($edge, $win)

  # ">_" prompt
  $w = [Math]::Max(1.5, 22 * $s)
  $pen = New-Object Drawing.Pen $fg, $w
  $pen.StartCap = 'Round'; $pen.EndCap = 'Round'; $pen.LineJoin = 'Round'
  $cx = 62 * $s; $cy = 150 * $s; $h = 34 * $s
  $g.DrawLines($pen, [Drawing.PointF[]]@(
    (New-Object Drawing.PointF $cx, ($cy - $h)),
    (New-Object Drawing.PointF ($cx + 40 * $s), $cy),
    (New-Object Drawing.PointF $cx, ($cy + $h))))
  $pen2 = New-Object Drawing.Pen $orange, $w
  $pen2.StartCap = 'Round'; $pen2.EndCap = 'Round'
  $g.DrawLine($pen2, (120 * $s), ($cy + $h), (182 * $s), ($cy + $h))

  $g.Dispose()
  $bmp
}

$sizes = 16, 24, 32, 48, 64, 128, 256
$pngs = foreach ($sz in $sizes) {
  $b = Draw $sz
  $ms = New-Object IO.MemoryStream
  $b.Save($ms, [Drawing.Imaging.ImageFormat]::Png)
  if ($sz -eq 256) { $b.Save((Join-Path $outDir 'icon.png'), [Drawing.Imaging.ImageFormat]::Png) }
  $b.Dispose()
  , $ms.ToArray()
}

# ICO container: header, directory entries, then PNG blobs
$out = New-Object IO.MemoryStream
$bw = New-Object IO.BinaryWriter $out
$bw.Write([uint16]0); $bw.Write([uint16]1); $bw.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
  $sz = $sizes[$i]; $len = $pngs[$i].Length
  $bw.Write([byte]($sz % 256)); $bw.Write([byte]($sz % 256))
  $bw.Write([byte]0); $bw.Write([byte]0)
  $bw.Write([uint16]1); $bw.Write([uint16]32)
  $bw.Write([uint32]$len); $bw.Write([uint32]$offset)
  $offset += $len
}
foreach ($p in $pngs) { $bw.Write($p) }
$bw.Flush()
[IO.File]::WriteAllBytes((Join-Path $outDir 'icon.ico'), $out.ToArray())
"wrote icon.ico ($($out.Length) bytes) and icon.png"
