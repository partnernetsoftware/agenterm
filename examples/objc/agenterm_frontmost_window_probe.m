/*
 * Independent black-box witness for the macOS window-activate court.
 *
 * This executable is not linked to agenterm-cu or libagenterm. It asks
 * NSWorkspace for the frontmost process, then selects that process's first
 * ordinary on-screen CoreGraphics window in WindowServer stacking order.
 * Only the numeric pid and CGWindowID are printed; user window titles never
 * enter the retained court bundle.
 */
#import <Cocoa/Cocoa.h>
#import <CoreGraphics/CoreGraphics.h>
#include <stdio.h>

int main(void) {
    @autoreleasepool {
        NSRunningApplication *frontmost =
            [[NSWorkspace sharedWorkspace] frontmostApplication];
        if (frontmost == nil) {
            return 2;
        }
        pid_t pid = [frontmost processIdentifier];
        CFArrayRef raw = CGWindowListCopyWindowInfo(
            kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
            kCGNullWindowID);
        if (raw == NULL) {
            return 3;
        }
        NSArray *windows = CFBridgingRelease(raw);
        for (NSDictionary *window in windows) {
            NSNumber *owner = window[(id)kCGWindowOwnerPID];
            NSNumber *layer = window[(id)kCGWindowLayer];
            NSNumber *number = window[(id)kCGWindowNumber];
            NSDictionary *bounds = window[(id)kCGWindowBounds];
            if (owner == nil || layer == nil || number == nil || bounds == nil) {
                continue;
            }
            if ([owner intValue] != pid || [layer intValue] != 0) {
                continue;
            }
            CGRect rect = CGRectZero;
            if (!CGRectMakeWithDictionaryRepresentation(
                    (__bridge CFDictionaryRef)bounds, &rect)
                || rect.size.width <= 0.0 || rect.size.height <= 0.0) {
                continue;
            }
            NSDictionary *result = @{
                @"pid" : @(pid),
                @"handle" : number,
            };
            NSData *json = [NSJSONSerialization dataWithJSONObject:result
                                                           options:0
                                                             error:nil];
            if (json == nil) {
                return 4;
            }
            fwrite([json bytes], 1, [json length], stdout);
            fputc('\n', stdout);
            return 0;
        }
        return 5;
    }
}
