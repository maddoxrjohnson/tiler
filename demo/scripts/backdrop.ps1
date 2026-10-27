# A plain dark window behind everything, so the recording shows no desktop.
# It is a tool window that never takes focus, so tiler leaves it alone.
Add-Type -ReferencedAssemblies System.Windows.Forms, System.Drawing -TypeDefinition @"
using System.Windows.Forms;
public class Backdrop : Form {
  // A tool window that never takes focus: tiler leaves it alone.
  protected override CreateParams CreateParams { get { var cp = base.CreateParams; cp.ExStyle |= 0x80 | 0x08000000; return cp; } }
  protected override bool ShowWithoutActivation { get { return true; } }
}
"@
$f = New-Object Backdrop
$f.FormBorderStyle = 'None'; $f.ShowInTaskbar = $false; $f.WindowState = 'Maximized'
$f.BackColor = [System.Drawing.Color]::FromArgb(24, 24, 27)
[System.Windows.Forms.Application]::Run($f)
