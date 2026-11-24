# Generates the built-in sticker PNGs (original flat artwork, MIT OR Apache-2.0).
# Usage (Windows PowerShell): powershell -File scripts/generate-stickers.ps1
Add-Type -AssemblyName System.Drawing
$out = Join-Path $PSScriptRoot '..\crates\doc\assets\stickers'
$S = 320      # output size in pixels; artwork is drawn on a 256 grid and scaled
function C($hex) { [Drawing.ColorTranslator]::FromHtml($hex) }
function Brush($hex) { New-Object Drawing.SolidBrush (C $hex) }
function Pen($hex, $w) { $p = New-Object Drawing.Pen (C $hex), $w; $p.StartCap = 'Round'; $p.EndCap = 'Round'; $p.LineJoin = 'Round'; $p }
function RoundRect($x, $y, $w, $h, $r) {
  $p = New-Object Drawing.Drawing2D.GraphicsPath
  $d = $r * 2
  $p.AddArc($x, $y, $d, $d, 180, 90); $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
  $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90); $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
  $p.CloseFigure(); $p
}
function Draw($name, [scriptblock]$body) {
  $bmp = New-Object Drawing.Bitmap $S, $S, ([Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $g = [Drawing.Graphics]::FromImage($bmp)
  $g.SmoothingMode = 'AntiAlias'; $g.Clear([Drawing.Color]::Transparent); $g.ScaleTransform($S / 256.0, $S / 256.0)
  & $body $g
  $g.Dispose(); $bmp.Save((Join-Path $out "$name.png"), [Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
}
function Pts($arr) { $arr | ForEach-Object { New-Object Drawing.PointF $_[0], $_[1] } }

Draw 'reminder_note' { param($g)
  $g.FillPath((Brush '#F8D96B'), (RoundRect 36 36 184 184 18))
  $g.FillPolygon((Brush '#E3B93C'), [Drawing.PointF[]](Pts @(@(220,170),@(170,220),@(220,220))))
  foreach ($y in 90,125,160) { $g.DrawLine((Pen '#9A7B1D' 8), 64, $y, 190, $y) } }
Draw 'todo_checklist' { param($g)
  $g.FillPath((Brush '#FFFFFF'), (RoundRect 40 30 176 196 18)); $g.DrawPath((Pen '#C9CED6' 6), (RoundRect 40 30 176 196 18))
  $y = 78; foreach ($i in 1..3) { $g.DrawRectangle((Pen '#7A8394' 6), 64, $y - 16, 32, 32); $g.DrawLine((Pen '#7A8394' 8), 112, $y, 196, $y); $y += 52 }
  $g.DrawLines((Pen '#22A06B' 8), [Drawing.PointF[]](Pts @(@(70,78),@(78,88),@(92,68)))) }
Draw 'important_alert' { param($g)
  $g.FillPolygon((Brush '#E5484D'), [Drawing.PointF[]](Pts @(@(128,26),@(236,214),@(20,214))))
  $g.FillPath((Brush '#FFFFFF'), (RoundRect 116 90 24 66 10)); $g.FillEllipse((Brush '#FFFFFF'), 114, 168, 28, 28) }
Draw 'done_stamp' { param($g)
  $g.FillEllipse((Brush '#2EA66B'), 28, 28, 200, 200)
  $g.DrawLines((Pen '#FFFFFF' 22), [Drawing.PointF[]](Pts @(@(78,132),@(112,166),@(180,92)))) }
Draw 'deadline_clock' { param($g)
  $g.FillEllipse((Brush '#FFFFFF'), 34, 34, 188, 188); $g.DrawEllipse((Pen '#E5484D' 14), 34, 34, 188, 188)
  $g.DrawLine((Pen '#33384A' 10), 128, 128, 128, 74); $g.DrawLine((Pen '#33384A' 10), 128, 128, 170, 150)
  $g.FillEllipse((Brush '#33384A'), 118, 118, 20, 20) }
Draw 'key_note' { param($g)
  $g.DrawEllipse((Pen '#E0A82E' 20), 40, 70, 90, 90)
  $g.DrawLine((Pen '#E0A82E' 20), 124, 128, 220, 128); $g.DrawLine((Pen '#E0A82E' 18), 190, 128, 190, 164); $g.DrawLine((Pen '#E0A82E' 18), 218, 128, 218, 156) }
Draw 'question_note' { param($g)
  $g.FillPath((Brush '#3E7BFA'), (RoundRect 30 30 196 196 36))
  $f = New-Object Drawing.Font 'Arial', 120, ([Drawing.FontStyle]::Bold), ([Drawing.GraphicsUnit]::Pixel)
  $fmt = New-Object Drawing.StringFormat; $fmt.Alignment = 'Center'; $fmt.LineAlignment = 'Center'
  $g.DrawString('?', $f, (Brush '#FFFFFF'), (New-Object Drawing.RectangleF 30, 30, 196, 206), $fmt) }
Draw 'idea_bulb' { param($g)
  $g.FillEllipse((Brush '#FFD84D'), 58, 24, 140, 140); $g.FillPolygon((Brush '#FFD84D'), [Drawing.PointF[]](Pts @(@(88,130),@(168,130),@(150,176),@(106,176))))
  $g.FillPath((Brush '#8B93A5'), (RoundRect 100 180 56 16 6)); $g.FillPath((Brush '#8B93A5'), (RoundRect 108 202 40 14 6))
  $g.DrawLines((Pen '#B88A00' 8), [Drawing.PointF[]](Pts @(@(106,110),@(128,86),@(150,110)))) }
Draw 'gold_star' { param($g)
  $pts = @(); foreach ($i in 0..9) { $r = if ($i % 2 -eq 0) { 108 } else { 46 }; $a = [Math]::PI / 5 * $i - [Math]::PI / 2; $pts += ,@((128 + $r * [Math]::Cos($a)), (136 + $r * [Math]::Sin($a))) }
  $g.FillPolygon((Brush '#F5B82E'), [Drawing.PointF[]](Pts $pts)); $g.DrawPolygon((Pen '#D18A00' 8), [Drawing.PointF[]](Pts $pts)) }
Draw 'streak_fire' { param($g)
  $p = New-Object Drawing.Drawing2D.GraphicsPath
  $p.AddBezier(128,20, 70,90, 50,140, 70,184); $p.AddBezier(70,184, 90,232, 166,232, 186,184); $p.AddBezier(186,184, 206,140, 160,110, 128,20)
  $g.FillPath((Brush '#F97316'), $p)
  $q = New-Object Drawing.Drawing2D.GraphicsPath; $q.AddBezier(128,110, 100,150, 100,190, 128,214); $q.AddBezier(128,214, 156,190, 156,150, 128,110)
  $g.FillPath((Brush '#FDD34D'), $q) }
Draw 'rocket_boost' { param($g)
  $g.FillPolygon((Brush '#F97316'), [Drawing.PointF[]](Pts @(@(104,176),@(152,176),@(128,236))))
  $g.FillPolygon((Brush '#E5484D'), [Drawing.PointF[]](Pts @(@(86,150),@(60,196),@(100,176)))); $g.FillPolygon((Brush '#E5484D'), [Drawing.PointF[]](Pts @(@(170,150),@(196,196),@(156,176))))
  $g.FillEllipse((Brush '#EEF1F6'), 86, 20, 84, 170); $g.DrawEllipse((Pen '#9AA3B5' 6), 86, 20, 84, 170)
  $g.FillEllipse((Brush '#3E7BFA'), 106, 70, 44, 44) }
Draw 'study_cat' { param($g)
  $g.FillPolygon((Brush '#9AA3B5'), [Drawing.PointF[]](Pts @(@(52,100),@(60,28),@(112,64))))
  $g.FillPolygon((Brush '#9AA3B5'), [Drawing.PointF[]](Pts @(@(204,100),@(196,28),@(144,64))))
  $g.FillEllipse((Brush '#B5BCCB'), 40, 56, 176, 160)
  $g.FillEllipse((Brush '#33384A'), 90, 116, 24, 32); $g.FillEllipse((Brush '#33384A'), 142, 116, 24, 32)
  $g.FillPolygon((Brush '#E58AA0'), [Drawing.PointF[]](Pts @(@(118,160),@(138,160),@(128,174))))
  $g.DrawLine((Pen '#33384A' 5), 128, 174, 128, 188); $g.DrawArc((Pen '#33384A' 5), 104, 178, 24, 18, 0, 180); $g.DrawArc((Pen '#33384A' 5), 128, 178, 24, 18, 0, 180) }
Draw 'coffee_cup' { param($g)
  $g.FillPath((Brush '#B5651D'), (RoundRect 50 100 130 110 24)); $g.DrawArc((Pen '#B5651D' 16), 150, 120, 60, 60, -90, 180)
  $g.FillPath((Brush '#8B93A5'), (RoundRect 36 204 158 16 8))
  foreach ($x in 84,114,144) { $g.DrawBezier((Pen '#B8C0D0' 7), $x, 90, ($x - 14), 70, ($x + 14), 52, $x, 30) } }
Draw 'desk_plant' { param($g)
  $g.FillPolygon((Brush '#D9792B'), [Drawing.PointF[]](Pts @(@(76,164),@(180,164),@(164,230),@(92,230))))
  $g.FillEllipse((Brush '#2EA66B'), 70, 60, 54, 110); $g.FillEllipse((Brush '#3FBF7F'), 112, 28, 56, 130); $g.FillEllipse((Brush '#2EA66B'), 140, 72, 54, 100) }
Draw 'cosmic_planet' { param($g)
  $g.FillEllipse((Brush '#7C5CE0'), 56, 56, 144, 144)
  $g.DrawArc((Pen '#F5B82E' 12), 14, 100, 228, 56, 180, 180); $g.FillEllipse((Brush '#7C5CE0'), 56, 56, 144, 144) | Out-Null
  $g.DrawArc((Pen '#F5B82E' 12), 14, 100, 228, 56, 0, 180) }