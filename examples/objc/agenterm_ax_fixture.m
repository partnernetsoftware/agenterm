/*
 * agenterm_ax_fixture.m -- a minimal Cocoa window OWNED BY THIS CHILD
 * process, deliberately independent of libagenterm.
 *
 * The macOS computer-use journey (scripts/qjs/cu-macos-smoke.qjs) needs a
 * window whose process it spawned and can reap, with a known accessibility
 * hierarchy: one AXWindow titled `agenterm-ax-fixture-<pid>`, one AXTextArea
 * (NSTextView, AXIdentifier `fixture-text`) seeded with `345AXTREE`, and one
 * AXButton titled `Fixture Press` (AXIdentifier `fixture-press`). That is
 * the hierarchy PRD_02_30's macOS recipe names.
 *
 * System applications cannot stand in: since macOS 13 a directly exec'd
 * system app binary is killed by launch constraints, and `open -a` hands the
 * process to LaunchServices, so its pid is not the spawner's to kill.
 *
 * Behaviour:
 *   - accessory activation policy: no Dock icon, and the window is ordered
 *     front WITHOUT activating, so the user's foreground app and key focus
 *     are untouched (the cu background invariant);
 *   - prints `ready <pid>` once the window is on screen, flushed, so the
 *     parent waits on that line instead of sleeping;
 *   - SIGTERM ends the process with exit code 0 (the journey's terminal
 *     state); the fixture never exits on its own.
 *
 * Build (the journey does this itself):
 *   clang -fobjc-arc -framework Cocoa -Wall -Wextra -Werror \
 *         examples/objc/agenterm_ax_fixture.m -o agenterm_ax_fixture
 */
#import <Cocoa/Cocoa.h>
#include <signal.h>
#include <stdio.h>
#include <unistd.h>

static void handle_terminate(int signal_number) {
    (void)signal_number;
    _exit(0);
}

int main(int argc, const char *argv[]) {
    (void)argc;
    (void)argv;
    signal(SIGTERM, handle_terminate);
    signal(SIGINT, handle_terminate);
    @autoreleasepool {
        NSApplication *app = [NSApplication sharedApplication];
        [app setActivationPolicy:NSApplicationActivationPolicyAccessory];

        NSString *title =
            [NSString stringWithFormat:@"agenterm-ax-fixture-%d", (int)getpid()];
        NSRect frame = NSMakeRect(160.0, 160.0, 480.0, 320.0);
        NSWindow *window = [[NSWindow alloc]
            initWithContentRect:frame
                      styleMask:(NSWindowStyleMaskTitled | NSWindowStyleMaskClosable)
                        backing:NSBackingStoreBuffered
                          defer:NO];
        [window setTitle:title];
        [window setReleasedWhenClosed:NO];
        NSView *content = [window contentView];

        NSScrollView *scroll =
            [[NSScrollView alloc] initWithFrame:NSMakeRect(20.0, 90.0, 440.0, 200.0)];
        NSTextView *text =
            [[NSTextView alloc] initWithFrame:NSMakeRect(0.0, 0.0, 440.0, 200.0)];
        [text setString:@"345AXTREE"];
        [text setAccessibilityIdentifier:@"fixture-text"];
        [scroll setDocumentView:text];
        [content addSubview:scroll];

        NSButton *button =
            [[NSButton alloc] initWithFrame:NSMakeRect(20.0, 24.0, 180.0, 40.0)];
        [button setTitle:@"Fixture Press"];
        [button setBezelStyle:NSBezelStyleRounded];
        [button setAccessibilityIdentifier:@"fixture-press"];
        [content addSubview:button];

        /* Order front without activating: the fixture must never take the
         * user's foreground or key focus. */
        [window orderFrontRegardless];

        printf("ready %d\n", (int)getpid());
        fflush(stdout);
        [app run];
    }
    return 0;
}
