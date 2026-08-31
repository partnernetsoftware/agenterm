# Owned WinForms fixture for the agenterm-cu Windows UIA journey.
#
# WinForms rather than a compiled Win32 program on purpose: this guest has
# no C toolchain, and a fixture the journey cannot build is a fixture the
# journey cannot run. These are real Win32 controls with real UI Automation
# providers, and its menu is a real Win32 HMENU.
#
# Two top-level forms, so the destructive close gate has a victim that is
# not the window every other step reads. Titles carry this process's pid so
# two runs cannot see each other's windows. Prints `ready <pid>` once both
# are up, then runs until closed.
#
# The labels keep their default accessible name: for a Label that name *is*
# the displayed text, so naming them would hide the value being read back.
# The interactive controls are named explicitly, because a journey that
# addresses nodes by --name cannot use an unnamed one.
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()

$pid_ = [System.Diagnostics.Process]::GetCurrentProcess().Id
$script:presses = 0
$script:things  = 0

$main = New-Object System.Windows.Forms.Form
$main.Text = "agenterm-win-fixture-$pid_"
$main.Size = New-Object System.Drawing.Size(420, 320)
$main.StartPosition = "Manual"
$main.Location = New-Object System.Drawing.Point(80, 80)

# A classic Win32 menu (MainMenu -> a real HMENU), not the newer
# MenuStrip. Measured on this guest: a MenuStrip publishes its menu bar to
# UI Automation but **no items at all while the menu is closed** -- the
# lazy-population case the product already refuses to force open. A real
# HMENU is published through the MSAA bridge with its items present, which
# is what makes a background menu walk observable here at all.
$menu = New-Object System.Windows.Forms.MainMenu
$file = New-Object System.Windows.Forms.MenuItem
$file.Text = "File"
$doThing = New-Object System.Windows.Forms.MenuItem
$doThing.Text = "Do Thing"
$disabled = New-Object System.Windows.Forms.MenuItem
$disabled.Text = "Disabled Thing"
$disabled.Enabled = $false
$marked = New-Object System.Windows.Forms.MenuItem
$marked.Text = "Marked Thing"
[void]$file.MenuItems.Add($doThing)
[void]$file.MenuItems.Add($disabled)
[void]$file.MenuItems.Add($marked)
[void]$menu.MenuItems.Add($file)
$main.Menu = $menu

$pressLabel = New-Object System.Windows.Forms.Label
$pressLabel.Text = "pressed 0"
$pressLabel.Location = New-Object System.Drawing.Point(20, 20)
$pressLabel.AutoSize = $true

$menuLabel = New-Object System.Windows.Forms.Label
$menuLabel.Text = "menu idle"
$menuLabel.Location = New-Object System.Drawing.Point(20, 50)
$menuLabel.AutoSize = $true

$entry = New-Object System.Windows.Forms.TextBox
$entry.Text = "seed"
$entry.AccessibleName = "Fixture Entry"
$entry.Location = New-Object System.Drawing.Point(20, 80)
$entry.Size = New-Object System.Drawing.Size(240, 24)

$check = New-Object System.Windows.Forms.CheckBox
$check.Text = "Fixture Check"
$check.AccessibleName = "Fixture Check"
$check.Location = New-Object System.Drawing.Point(20, 120)
$check.AutoSize = $true

$button = New-Object System.Windows.Forms.Button
$button.Text = "Fixture Press"
$button.AccessibleName = "Fixture Press"
$button.Location = New-Object System.Drawing.Point(20, 155)
$button.Size = New-Object System.Drawing.Size(160, 30)

$button.Add_Click({
    $script:presses++
    $pressLabel.Text = "pressed $($script:presses)"
})
$doThing.Add_Click({
    $script:things++
    $menuLabel.Text = "did thing $($script:things)"
})

[void]$main.Controls.Add($pressLabel)
[void]$main.Controls.Add($menuLabel)
[void]$main.Controls.Add($entry)
[void]$main.Controls.Add($check)
[void]$main.Controls.Add($button)

$second = New-Object System.Windows.Forms.Form
$second.Text = "agenterm-win-second-$pid_"
$second.Size = New-Object System.Drawing.Size(260, 180)
$second.StartPosition = "Manual"
$second.Location = New-Object System.Drawing.Point(560, 80)
$secondLabel = New-Object System.Windows.Forms.Label
$secondLabel.Text = "second window"
$secondLabel.Location = New-Object System.Drawing.Point(20, 30)
$secondLabel.AutoSize = $true
[void]$second.Controls.Add($secondLabel)

$main.Add_Shown({
    # Announce only once both windows exist, so a journey that waits on the
    # title cannot match before the second one is there to close.
    [Console]::Out.WriteLine("ready $pid_")
    [Console]::Out.Flush()
})

$second.Show()
$main.Show()
$main.Activate()
[System.Windows.Forms.Application]::Run()
