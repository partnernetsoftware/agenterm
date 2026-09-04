/*
 * Owned, non-sensitive NSSavePanel fixture for the agenterm-cu macOS court.
 *
 * Usage: agenterm_save_panel_fixture --save-panel DIRECTORY FILENAME
 *
 * The fixture owns the directory and writes a fixed harmless payload only
 * after AppKit reports NSModalResponseOK. Cancel writes nothing. SIGTERM and
 * SIGINT terminate the process so the journey can always reap its child.
 */
#import <Cocoa/Cocoa.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

static void handle_terminate(int signal_number) {
    (void)signal_number;
    _exit(0);
}

int main(int argc, const char *argv[]) {
    if (argc != 4 || strcmp(argv[1], "--save-panel") != 0) {
        fprintf(stderr, "usage: agenterm_save_panel_fixture --save-panel DIRECTORY FILENAME\n");
        return 2;
    }
    signal(SIGTERM, handle_terminate);
    signal(SIGINT, handle_terminate);

    @autoreleasepool {
        NSString *directory = [NSString stringWithUTF8String:argv[2]];
        NSString *filename = [NSString stringWithUTF8String:argv[3]];
        if (directory == nil || filename == nil || [filename length] == 0
            || [filename containsString:@"/"]) {
            fprintf(stderr, "invalid save-panel path\n");
            return 2;
        }

        NSApplication *app = [NSApplication sharedApplication];
        [app setActivationPolicy:NSApplicationActivationPolicyAccessory];

        NSWindow *owner = [[NSWindow alloc]
            initWithContentRect:NSMakeRect(240.0, 240.0, 420.0, 180.0)
                      styleMask:(NSWindowStyleMaskTitled | NSWindowStyleMaskClosable)
                        backing:NSBackingStoreBuffered
                          defer:NO];
        [owner setTitle:[NSString stringWithFormat:@"agenterm-save-panel-owner-%d",
                                                   (int)getpid()]];
        [owner setReleasedWhenClosed:NO];
        NSTextField *label = [[NSTextField alloc]
            initWithFrame:NSMakeRect(24.0, 72.0, 372.0, 28.0)];
        [label setStringValue:@"Owned harmless Save Panel fixture"];
        [label setEditable:NO];
        [label setBezeled:NO];
        [label setDrawsBackground:NO];
        [[owner contentView] addSubview:label];
        [owner orderFrontRegardless];

        NSSavePanel *panel = [NSSavePanel savePanel];
        [panel setDirectoryURL:[NSURL fileURLWithPath:directory isDirectory:YES]];
        [panel setNameFieldStringValue:filename];
        [panel setCanCreateDirectories:NO];
        [panel setShowsTagField:NO];
        [panel beginSheetModalForWindow:owner completionHandler:^(NSModalResponse response) {
            if (response == NSModalResponseOK) {
                NSData *payload = [@"agenterm-save-panel-smoke\n"
                    dataUsingEncoding:NSUTF8StringEncoding];
                if (![payload writeToURL:[panel URL] options:NSDataWritingAtomic error:nil]) {
                    fprintf(stderr, "fixture write failed\n");
                }
            }
        }];

        printf("ready %d %s\n", (int)getpid(), argv[3]);
        fflush(stdout);
        [app run];
    }
    return 0;
}
