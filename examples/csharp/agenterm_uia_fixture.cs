// Owned WinForms fixture for the agenterm-cu Windows UIA journey.
//
// This is source, not a checked-in binary. The .NET Framework csc.exe already
// present on Windows compiles it, so the journey needs neither an external C
// toolchain nor a PowerShell source/runtime.
using System;
using System.Diagnostics;
using System.Drawing;
using System.Windows.Forms;

public static class AgentermUiaFixture
{
    private static int presses;
    private static int things;

    [STAThread]
    public static void Main()
    {
        Run();
    }

    public static void Run()
    {
        Application.EnableVisualStyles();

        int pid = Process.GetCurrentProcess().Id;
        var main = new Form {
            Text = "agenterm-win-fixture-" + pid,
            Size = new Size(420, 320),
            StartPosition = FormStartPosition.Manual,
            Location = new Point(80, 80)
        };

        // MainMenu produces a real HMENU. Unlike MenuStrip on the qualification
        // court, its closed top-level item is visible through the MSAA bridge.
        var menu = new MainMenu();
        var file = new MenuItem("File");
        var doThing = new MenuItem("Do Thing");
        var disabled = new MenuItem("Disabled Thing") { Enabled = false };
        var marked = new MenuItem("Marked Thing");
        file.MenuItems.Add(doThing);
        file.MenuItems.Add(disabled);
        file.MenuItems.Add(marked);
        menu.MenuItems.Add(file);
        main.Menu = menu;

        var pressLabel = new Label {
            Text = "pressed 0", Location = new Point(20, 20), AutoSize = true
        };
        var menuLabel = new Label {
            Text = "menu idle", Location = new Point(20, 50), AutoSize = true
        };
        var entry = new TextBox {
            Text = "seed", AccessibleName = "Fixture Entry",
            Location = new Point(20, 80), Size = new Size(240, 24)
        };
        var check = new CheckBox {
            Text = "Fixture Check", AccessibleName = "Fixture Check",
            Location = new Point(20, 120), AutoSize = true
        };
        var button = new Button {
            Text = "Fixture Press", AccessibleName = "Fixture Press",
            Location = new Point(20, 155), Size = new Size(160, 30)
        };

        button.Click += delegate {
            presses += 1;
            pressLabel.Text = "pressed " + presses;
        };
        doThing.Click += delegate {
            things += 1;
            menuLabel.Text = "did thing " + things;
        };

        main.Controls.Add(pressLabel);
        main.Controls.Add(menuLabel);
        main.Controls.Add(entry);
        main.Controls.Add(check);
        main.Controls.Add(button);

        var second = new Form {
            Text = "agenterm-win-second-" + pid,
            Size = new Size(260, 180),
            StartPosition = FormStartPosition.Manual,
            Location = new Point(560, 80)
        };
        second.Controls.Add(new Label {
            Text = "second window", Location = new Point(20, 30), AutoSize = true
        });

        main.Shown += delegate {
            Console.Out.WriteLine("ready " + pid);
            Console.Out.Flush();
        };

        second.Show();
        main.Show();
        main.Activate();
        Application.Run();
    }
}
