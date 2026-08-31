/*
 * agenterm_web_fixture.m -- an OWNED WKWebView window for the macOS web
 * accessibility journey (scripts/qjs/cu-macos-web-smoke.qjs).
 *
 * The AX default loop reaches web content through the system tree: a
 * browser publishes an AXWebArea whose descendants are the page's own
 * headings, links, buttons and fields. Proving that against a real browser
 * would mean driving the user's Brave or Chrome, so this fixture owns the
 * page instead: one accessory-policy window holding a WKWebView with a
 * hermetic `loadHTMLString` document (no network, no profile, no cache).
 *
 * The page is shaped by what the journey has to prove:
 *   - `<h1 id=fixture-web-heading>` and `<p>`: AXHeading / AXStaticText
 *     reads inside the WebArea;
 *   - `<button id=fixture-web-button>`: `invoke press` on a web node, with
 *     `<span id=fixture-web-count>` as its postcondition on another node;
 *   - `<input id=fixture-web-field>` seeded `web seed`: `invoke set-value`
 *     and `verify` on a web node;
 *   - a 1200px spacer and then `<a id=fixture-web-deep>`: a link far below
 *     the fold, so `scroll` has something to bring into view and
 *     `get-extents` can read the movement back. AppKit publishes
 *     `AXScrollToVisible` on nothing a plain Cocoa control can hold, but
 *     every WebKit node offers it -- this fixture is where cu's `scroll`
 *     gets its positive evidence.
 *
 * The window uses `orderFrontRegardless` and the accessory activation
 * policy, so it never becomes the frontmost application and the journey's
 * background invariant holds. SIGTERM ends the process with exit 0.
 *
 * Build (the journey does this itself):
 *   clang -fobjc-arc -framework Cocoa -framework WebKit -Wall -Wextra \
 *         -Werror examples/objc/agenterm_web_fixture.m -o agenterm_web_fixture
 */
#import <Cocoa/Cocoa.h>
#import <WebKit/WebKit.h>
#include <signal.h>
#include <stdio.h>
#include <unistd.h>

static void handle_terminate(int signal_number) {
    (void)signal_number;
    _exit(0);
}

static NSString *fixture_html(void) {
    return @"<!doctype html><html><head><meta charset='utf-8'>"
            "<title>agenterm web fixture</title></head><body>"
            "<h1 id='fixture-web-heading'>Fixture Web Heading</h1>"
            "<p id='fixture-web-para'>fixture web paragraph</p>"
            "<button id='fixture-web-button' onclick=\"var c=document.getElementById("
            "'fixture-web-count'); c.textContent='web pressed '+(++window.n);\">"
            "Fixture Web Button</button>"
            "<p><span id='fixture-web-count'>web pressed 0</span></p>"
            "<p><input id='fixture-web-field' aria-label='Fixture Web Field' "
            "value='web seed'></p>"
            "<div style='height:1200px'></div>"
            "<p><a id='fixture-web-deep' href='#fixture-web-deep'>Fixture Deep Link</a></p>"
            "<script>window.n=0;</script></body></html>";
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
            [NSString stringWithFormat:@"agenterm-web-fixture-%d", (int)getpid()];
        NSRect frame = NSMakeRect(820.0, 140.0, 520.0, 420.0);
        NSWindow *window = [[NSWindow alloc]
            initWithContentRect:frame
                      styleMask:(NSWindowStyleMaskTitled | NSWindowStyleMaskClosable
                                 | NSWindowStyleMaskResizable)
                        backing:NSBackingStoreBuffered
                          defer:NO];
        [window setTitle:title];
        [window setReleasedWhenClosed:NO];

        WKWebView *web =
            [[WKWebView alloc] initWithFrame:NSMakeRect(0.0, 0.0, 520.0, 420.0)];
        [web setAccessibilityIdentifier:@"fixture-web-view"];
        [web loadHTMLString:fixture_html() baseURL:nil];
        [[window contentView] addSubview:web];

        /* Front without activating: the user's frontmost application and
         * the cu background invariant are both untouched. */
        [window orderFrontRegardless];

        printf("ready %d\n", (int)getpid());
        fflush(stdout);
        [app run];
    }
    return 0;
}
