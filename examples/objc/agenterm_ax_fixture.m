/*
 * agenterm_ax_fixture.m -- a minimal Cocoa window OWNED BY THIS CHILD
 * process, deliberately independent of libagenterm.
 *
 * The macOS computer-use journey (scripts/qjs/cu-macos-smoke.qjs) needs a
 * window whose process it spawned and can reap, with a known accessibility
 * hierarchy: one AXWindow titled `agenterm-ax-fixture-<pid>`, one AXTextArea
 * (NSTextView, AXIdentifier `fixture-text`) seeded with `345AXTREE`, and one
 * AXButton titled `Fixture Press` (AXIdentifier `fixture-press`). That is
 * the hierarchy PRD_02_30's macOS recipe names (slice 1, observe).
 *
 * Slice 2 (actuation) adds the controls `invoke` / `verify` act on, every
 * one addressable by AXIdentifier and none of them needing key focus:
 *   - AXTextField `fixture-field` seeded with `seed` (set-value target);
 *   - AXCheckBox `Fixture Check` / `fixture-check`, initially off
 *     (set-checked target; the second identical call must be a no-op);
 *   - AXIncrementor (NSStepper) `fixture-stepper`, 0..10 starting at 3
 *     (increment / decrement target; its AXValue is the number);
 *   - AXPopUpButton `fixture-popup` with items Alpha / Beta / Gamma, Alpha
 *     selected (select-option target);
 *   - AXStaticText `fixture-press-count` reading `pressed 0`, which the
 *     `Fixture Press` button advances to `pressed 1`, `pressed 2`, ... so a
 *     press has a postcondition `verify` can read on another node;
 *   - two AXButtons both titled `Fixture Twin` (`fixture-twin-a` /
 *     `fixture-twin-b`) so a `--name` that matches both is a proven
 *     ambiguity refusal.
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

/* Target for the `Fixture Press` button: advances the press-count label so
 * the press has an observable postcondition on a different node. */
@interface AgentermFixtureController : NSObject
@property(nonatomic, strong) NSTextField *pressCount;
@property(nonatomic, assign) int presses;
- (void)press:(id)sender;
@end

@implementation AgentermFixtureController
- (void)press:(id)sender {
    (void)sender;
    self.presses = self.presses + 1;
    [self.pressCount setStringValue:[NSString stringWithFormat:@"pressed %d", self.presses]];
}
@end

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
        NSRect frame = NSMakeRect(160.0, 160.0, 560.0, 440.0);
        NSWindow *window = [[NSWindow alloc]
            initWithContentRect:frame
                      styleMask:(NSWindowStyleMaskTitled | NSWindowStyleMaskClosable)
                        backing:NSBackingStoreBuffered
                          defer:NO];
        [window setTitle:title];
        [window setReleasedWhenClosed:NO];
        NSView *content = [window contentView];

        NSScrollView *scroll =
            [[NSScrollView alloc] initWithFrame:NSMakeRect(20.0, 220.0, 520.0, 200.0)];
        NSTextView *text =
            [[NSTextView alloc] initWithFrame:NSMakeRect(0.0, 0.0, 520.0, 200.0)];
        [text setString:@"345AXTREE"];
        [text setAccessibilityIdentifier:@"fixture-text"];
        [scroll setDocumentView:text];
        [content addSubview:scroll];

        /* Slice 2 controls. */
        NSTextField *field =
            [[NSTextField alloc] initWithFrame:NSMakeRect(20.0, 176.0, 200.0, 24.0)];
        [field setStringValue:@"seed"];
        [field setAccessibilityIdentifier:@"fixture-field"];
        [content addSubview:field];

        NSButton *check =
            [[NSButton alloc] initWithFrame:NSMakeRect(240.0, 176.0, 150.0, 24.0)];
        [check setButtonType:NSButtonTypeSwitch];
        [check setTitle:@"Fixture Check"];
        [check setState:NSControlStateValueOff];
        [check setAccessibilityIdentifier:@"fixture-check"];
        [content addSubview:check];

        NSStepper *stepper =
            [[NSStepper alloc] initWithFrame:NSMakeRect(410.0, 172.0, 20.0, 30.0)];
        [stepper setMinValue:0.0];
        [stepper setMaxValue:10.0];
        [stepper setIncrement:1.0];
        [stepper setIntValue:3];
        [stepper setAutorepeat:NO];
        [stepper setValueWraps:NO];
        [stepper setAccessibilityIdentifier:@"fixture-stepper"];
        [content addSubview:stepper];

        NSPopUpButton *popup =
            [[NSPopUpButton alloc] initWithFrame:NSMakeRect(20.0, 130.0, 180.0, 28.0)
                                       pullsDown:NO];
        [popup addItemsWithTitles:@[ @"Alpha", @"Beta", @"Gamma" ]];
        [popup selectItemAtIndex:0];
        [popup setAccessibilityIdentifier:@"fixture-popup"];
        [content addSubview:popup];

        NSTextField *pressCount =
            [[NSTextField alloc] initWithFrame:NSMakeRect(240.0, 132.0, 200.0, 24.0)];
        [pressCount setStringValue:@"pressed 0"];
        [pressCount setEditable:NO];
        [pressCount setBezeled:NO];
        [pressCount setDrawsBackground:NO];
        [pressCount setSelectable:NO];
        [pressCount setAccessibilityIdentifier:@"fixture-press-count"];
        [content addSubview:pressCount];

        AgentermFixtureController *controller = [[AgentermFixtureController alloc] init];
        controller.pressCount = pressCount;
        controller.presses = 0;

        NSButton *twinA =
            [[NSButton alloc] initWithFrame:NSMakeRect(240.0, 76.0, 130.0, 32.0)];
        [twinA setTitle:@"Fixture Twin"];
        [twinA setBezelStyle:NSBezelStyleRounded];
        [twinA setAccessibilityIdentifier:@"fixture-twin-a"];
        [content addSubview:twinA];
        NSButton *twinB =
            [[NSButton alloc] initWithFrame:NSMakeRect(390.0, 76.0, 130.0, 32.0)];
        [twinB setTitle:@"Fixture Twin"];
        [twinB setBezelStyle:NSBezelStyleRounded];
        [twinB setAccessibilityIdentifier:@"fixture-twin-b"];
        [content addSubview:twinB];

        NSButton *button =
            [[NSButton alloc] initWithFrame:NSMakeRect(20.0, 24.0, 180.0, 40.0)];
        [button setTitle:@"Fixture Press"];
        [button setBezelStyle:NSBezelStyleRounded];
        [button setAccessibilityIdentifier:@"fixture-press"];
        [button setTarget:controller];
        [button setAction:@selector(press:)];
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
