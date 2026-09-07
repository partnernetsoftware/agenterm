/* Owned, non-activating wheel fixture for cu-pointer-scroll-smoke.qjs. */
#import <Cocoa/Cocoa.h>
#include <signal.h>
#include <stdio.h>
#include <unistd.h>

static NSString *statePath;
static NSInteger sequenceNumber = 0;
static double contentOffset = 500.0;

static void publishState(void) {
    NSString *json = [NSString stringWithFormat:
        @"{\"ready\":true,\"sequence\":%ld,\"content_offset\":%.3f}\n",
        (long)sequenceNumber, contentOffset];
    NSString *temporary = [statePath stringByAppendingString:@".tmp"];
    NSError *error = nil;
    if (![json writeToFile:temporary atomically:NO encoding:NSUTF8StringEncoding error:&error]) {
        _exit(21);
    }
    if (rename([temporary fileSystemRepresentation], [statePath fileSystemRepresentation]) != 0) {
        _exit(22);
    }
}

static void terminateFixture(int signalNumber) {
    (void)signalNumber;
    _exit(0);
}

@interface ScrollProbeView : NSView
@end

@implementation ScrollProbeView
- (void)scrollWheel:(NSEvent *)event {
    double delta = [event scrollingDeltaY];
    if (delta == 0.0) { delta = [event deltaY]; }
    contentOffset += delta;
    sequenceNumber += 1;
    publishState();
}
@end

int main(int argc, const char *argv[]) {
    if (argc != 2) { return 2; }
    signal(SIGTERM, terminateFixture);
    signal(SIGINT, terminateFixture);
    @autoreleasepool {
        statePath = [NSString stringWithUTF8String:argv[1]];
        NSApplication *application = [NSApplication sharedApplication];
        [application setActivationPolicy:NSApplicationActivationPolicyAccessory];
        NSPoint pointer = [NSEvent mouseLocation];
        NSRect frame = NSMakeRect(pointer.x - 120.0, pointer.y - 90.0, 240.0, 180.0);
        NSPanel *panel = [[NSPanel alloc]
            initWithContentRect:frame
                      styleMask:(NSWindowStyleMaskBorderless | NSWindowStyleMaskNonactivatingPanel)
                        backing:NSBackingStoreBuffered
                          defer:NO];
        [panel setLevel:NSStatusWindowLevel];
        [panel setHidesOnDeactivate:NO];
        [panel setOpaque:YES];
        [panel setBackgroundColor:[NSColor colorWithCalibratedWhite:0.12 alpha:1.0]];
        ScrollProbeView *view = [[ScrollProbeView alloc] initWithFrame:NSMakeRect(0, 0, 240, 180)];
        [panel setContentView:view];
        [panel orderFrontRegardless];
        publishState();
        [application run];
    }
    return 0;
}
