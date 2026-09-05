#import <AVFoundation/AVFoundation.h>
#import <CoreImage/CoreImage.h>
#import <CoreMediaIO/CMIOHardware.h>
#import <Foundation/Foundation.h>

#include "device_capture.h"
#include "usbmux_probe.h"

#include <limits.h>
#include <stdlib.h>
#include <string.h>

static NSString *const kAgentermIOSModel = @"iOS Device";
static const NSUInteger kAgentermMaxDeviceSources = 64;
static const size_t kAgentermMaxPngBytes = 64u * 1024u * 1024u;

static void CopyText(char *slot, size_t capacity, NSString *text) {
    if (!slot || capacity == 0) return;
    slot[0] = '\0';
    if (text) {
        [text getCString:slot maxLength:capacity encoding:NSUTF8StringEncoding];
    }
}

static int32_t CameraAuthorization(void) {
    switch ([AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeVideo]) {
        case AVAuthorizationStatusAuthorized:
            return AGENTERM_CAMERA_AUTHORIZED;
        case AVAuthorizationStatusDenied:
            return AGENTERM_CAMERA_DENIED;
        case AVAuthorizationStatusRestricted:
            return AGENTERM_CAMERA_RESTRICTED;
        case AVAuthorizationStatusNotDetermined:
        default:
            // Observation must never call requestAccessForMediaType. Treat an
            // SDK status unknown to this build as undecided, which also keeps
            // the stream closed until a later adapter understands it.
            return AGENTERM_CAMERA_NOT_DETERMINED;
    }
}

static OSStatus AllowScreenCaptureDevices(void) {
    CMIOObjectPropertyAddress address = {
        kCMIOHardwarePropertyAllowScreenCaptureDevices,
        kCMIOObjectPropertyScopeGlobal,
        kCMIOObjectPropertyElementMain,
    };
    UInt32 enabled = 1;
    return CMIOObjectSetPropertyData(kCMIOObjectSystemObject, &address, 0, NULL,
                                     sizeof(enabled), &enabled);
}

static NSArray<AVCaptureDevice *> *IOSCaptureDevices(NSString **failure) {
    OSStatus status = AllowScreenCaptureDevices();
    if (status != noErr) {
        if (failure) {
            *failure = [NSString stringWithFormat:
                                      @"CoreMediaIO refused device-source discovery (%d)",
                                      (int)status];
        }
        return nil;
    }

    @try {
        AVCaptureDeviceDiscoverySession *session = [AVCaptureDeviceDiscoverySession
            discoverySessionWithDeviceTypes:@[ AVCaptureDeviceTypeExternal ]
                                  mediaType:nil
                                   position:AVCaptureDevicePositionUnspecified];
        if (!session) {
            if (failure) *failure = @"AVFoundation did not create a device discovery session";
            return nil;
        }
        NSMutableArray<AVCaptureDevice *> *found = [NSMutableArray array];
        for (AVCaptureDevice *device in session.devices) {
            if ([device.modelID containsString:kAgentermIOSModel]) {
                [found addObject:device];
            }
        }
        return found;
    } @catch (NSException *exception) {
        if (failure) {
            *failure = [NSString stringWithFormat:@"AVFoundation device discovery failed: %@",
                                                  exception.reason ?: exception.name];
        }
        return nil;
    }
}

static void ObserveImpl(AgentermDeviceCaptureEvidence *out) {
    @autoreleasepool {
        out->camera_authorization = CameraAuthorization();

        AgentermUsbmuxObservation mux = agenterm_usbmux_observe();
        if (mux.available) {
            out->usbmux_status = AGENTERM_OBSERVATION_INVENTORY;
            out->connected_devices = mux.connected_devices;
            out->paired_devices = mux.paired_devices;
        } else {
            out->usbmux_status = AGENTERM_OBSERVATION_FAILED;
            CopyText(out->usbmux_message, sizeof(out->usbmux_message), @(mux.message));
        }

        NSString *dal_failure = nil;
        NSArray<AVCaptureDevice *> *devices = IOSCaptureDevices(&dal_failure);
        if (!devices) {
            out->dal_status = AGENTERM_OBSERVATION_FAILED;
            CopyText(out->dal_message, sizeof(out->dal_message),
                     dal_failure ?: @"DAL device inventory failed");
            return;
        }

        out->dal_status = AGENTERM_OBSERVATION_INVENTORY;
        if (devices.count == 0) return;
        if (devices.count > kAgentermMaxDeviceSources ||
            devices.count > SIZE_MAX / sizeof(AgentermDeviceCaptureSource)) {
            out->dal_status = AGENTERM_OBSERVATION_FAILED;
            CopyText(out->dal_message, sizeof(out->dal_message),
                     @"DAL device inventory exceeds the 64-source limit");
            return;
        }

        out->sources = calloc(devices.count, sizeof(AgentermDeviceCaptureSource));
        if (!out->sources) {
            out->dal_status = AGENTERM_OBSERVATION_FAILED;
            CopyText(out->dal_message, sizeof(out->dal_message),
                     @"cannot allocate DAL device inventory");
            return;
        }
        out->source_count = devices.count;
        for (NSUInteger index = 0; index < devices.count; index++) {
            AVCaptureDevice *device = devices[index];
            CopyText(out->sources[index].name, sizeof(out->sources[index].name),
                     device.localizedName ?: @"");
            CopyText(out->sources[index].uid, sizeof(out->sources[index].uid),
                     device.uniqueID ?: @"");
        }
    }
}

void agenterm_device_capture_observe(AgentermDeviceCaptureEvidence *out) {
    if (!out) return;
    memset(out, 0, sizeof(*out));
    out->camera_authorization = AGENTERM_CAMERA_NOT_DETERMINED;
    out->usbmux_status = AGENTERM_OBSERVATION_FAILED;
    out->dal_status = AGENTERM_OBSERVATION_FAILED;
    @autoreleasepool {
        @try {
            CopyText(out->usbmux_message, sizeof(out->usbmux_message),
                     @"native usbmux observation did not complete");
            CopyText(out->dal_message, sizeof(out->dal_message),
                     @"native DAL observation did not complete");
            ObserveImpl(out);
        } @catch (NSException *exception) {
            free(out->sources);
            out->sources = NULL;
            out->source_count = 0;
            out->dal_status = AGENTERM_OBSERVATION_FAILED;
            CopyText(out->dal_message, sizeof(out->dal_message),
                     [NSString stringWithFormat:@"native device observation failed: %@",
                                                exception.reason ?: exception.name]);
        }
    }
}

void agenterm_device_capture_evidence_free(AgentermDeviceCaptureEvidence *out) {
    if (!out) return;
    free(out->sources);
    out->sources = NULL;
    out->source_count = 0;
}

@interface AgentermDeviceFrameSink : NSObject <AVCaptureVideoDataOutputSampleBufferDelegate> {
    CVPixelBufferRef _frame;
}
@property(nonatomic, readonly) dispatch_semaphore_t ready;
- (CVPixelBufferRef)copyFrame;
@end

@implementation AgentermDeviceFrameSink
- (instancetype)init {
    self = [super init];
    if (self) _ready = dispatch_semaphore_create(0);
    return self;
}
- (void)captureOutput:(AVCaptureOutput *)output
    didOutputSampleBuffer:(CMSampleBufferRef)sample_buffer
           fromConnection:(AVCaptureConnection *)connection {
    (void)output;
    (void)connection;
    CVPixelBufferRef pixels = CMSampleBufferGetImageBuffer(sample_buffer);
    if (!pixels) return;
    @synchronized(self) {
        if (_frame) return;
        _frame = CVPixelBufferRetain(pixels);
    }
    dispatch_semaphore_signal(self.ready);
}
- (CVPixelBufferRef)copyFrame {
    @synchronized(self) {
        return _frame ? CVPixelBufferRetain(_frame) : NULL;
    }
}
- (void)dealloc {
    if (_frame) CVPixelBufferRelease(_frame);
}
@end

static void FailFrame(AgentermDeviceCaptureFrame *out, int32_t status,
                      NSString *message) {
    out->status = status;
    CopyText(out->message, sizeof(out->message), message);
}

static void CaptureSelectedImpl(const char *uid, uint64_t timeout_ms,
                                AgentermDeviceCaptureFrame *out) {
    @autoreleasepool {
        // Calling requestAccess here would create a TCC prompt on a worker or
        // automation path. Selection should already have checked this, but the
        // native seam independently fails closed if authorization changed.
        if (CameraAuthorization() != AGENTERM_CAMERA_AUTHORIZED) {
            FailFrame(out, AGENTERM_DEVICE_STREAM_OPEN_FAILED,
                      @"host Camera authorization is not currently granted");
            return;
        }
        if (!uid || uid[0] == '\0') {
            FailFrame(out, AGENTERM_DEVICE_STREAM_OPEN_FAILED,
                      @"the selected DAL source has no uid");
            return;
        }

        NSString *failure = nil;
        NSArray<AVCaptureDevice *> *devices = IOSCaptureDevices(&failure);
        if (!devices) {
            FailFrame(out, AGENTERM_DEVICE_STREAM_OPEN_FAILED,
                      failure ?: @"DAL device inventory failed before stream open");
            return;
        }
        NSString *wanted = [NSString stringWithUTF8String:uid];
        if (!wanted) {
            FailFrame(out, AGENTERM_DEVICE_STREAM_OPEN_FAILED,
                      @"the selected DAL uid is not valid UTF-8");
            return;
        }
        AVCaptureDevice *device = nil;
        for (AVCaptureDevice *candidate in devices) {
            if ([candidate.uniqueID isEqualToString:wanted]) {
                device = candidate;
                break;
            }
        }
        if (!device) {
            FailFrame(out, AGENTERM_DEVICE_STREAM_OPEN_FAILED,
                      @"the selected DAL source is no longer published");
            return;
        }

        NSError *input_error = nil;
        AVCaptureDeviceInput *input =
            [AVCaptureDeviceInput deviceInputWithDevice:device error:&input_error];
        if (!input) {
            FailFrame(out, AGENTERM_DEVICE_STREAM_OPEN_FAILED,
                      input_error.localizedDescription ?: @"cannot open the selected DAL source");
            return;
        }

        AVCaptureSession *session = [AVCaptureSession new];
        [session beginConfiguration];
        if (![session canAddInput:input]) {
            [session commitConfiguration];
            FailFrame(out, AGENTERM_DEVICE_STREAM_INPUT_REFUSED,
                      @"the capture session refused the selected device input");
            return;
        }
        [session addInput:input];

        AVCaptureVideoDataOutput *output = [AVCaptureVideoDataOutput new];
        output.videoSettings =
            @{(id)kCVPixelBufferPixelFormatTypeKey : @(kCVPixelFormatType_32BGRA)};
        output.alwaysDiscardsLateVideoFrames = YES;
        dispatch_queue_t queue =
            dispatch_queue_create("agenterm-platform.device-capture", DISPATCH_QUEUE_SERIAL);
        AgentermDeviceFrameSink *sink = [AgentermDeviceFrameSink new];
        [output setSampleBufferDelegate:sink queue:queue];
        if (![session canAddOutput:output]) {
            [output setSampleBufferDelegate:nil queue:NULL];
            [session commitConfiguration];
            FailFrame(out, AGENTERM_DEVICE_STREAM_OUTPUT_REFUSED,
                      @"the capture session refused the video-data output");
            return;
        }
        [session addOutput:output];
        [session commitConfiguration];

        [session startRunning];
        if (!session.running) {
            [output setSampleBufferDelegate:nil queue:NULL];
            FailFrame(out, AGENTERM_DEVICE_STREAM_START_FAILED,
                      @"the capture session did not start running");
            return;
        }

        uint64_t max_ms = (uint64_t)INT64_MAX / NSEC_PER_MSEC;
        uint64_t bounded_ms = timeout_ms > max_ms ? max_ms : timeout_ms;
        dispatch_time_t deadline =
            dispatch_time(DISPATCH_TIME_NOW, (int64_t)(bounded_ms * NSEC_PER_MSEC));
        long wait_status = dispatch_semaphore_wait(sink.ready, deadline);
        [session stopRunning];
        [output setSampleBufferDelegate:nil queue:NULL];
        dispatch_sync(queue, ^{});

        if (wait_status != 0) {
            // A timeout has several possible causes. AVFoundation supplied no
            // explicit lock or trust signal, so preserve the generic timeout.
            FailFrame(out, AGENTERM_DEVICE_STREAM_FRAME_TIMEOUT,
                      @"the selected capture source emitted no frame before the deadline");
            return;
        }

        CVPixelBufferRef frame = [sink copyFrame];
        if (!frame) {
            FailFrame(out, AGENTERM_DEVICE_STREAM_FRAME_TIMEOUT,
                      @"the selected capture source signaled without a readable frame");
            return;
        }
        size_t width = CVPixelBufferGetWidth(frame);
        size_t height = CVPixelBufferGetHeight(frame);
        if (width == 0 || height == 0 || width > UINT32_MAX || height > UINT32_MAX) {
            CVPixelBufferRelease(frame);
            FailFrame(out, AGENTERM_DEVICE_STREAM_ENCODE_FAILED,
                      @"the captured frame dimensions are outside supported bounds");
            return;
        }
        out->width = (uint32_t)width;
        out->height = (uint32_t)height;

        CIImage *image = [CIImage imageWithCVPixelBuffer:frame];
        CIContext *context = [CIContext context];
        CGColorSpaceRef srgb = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
        NSData *png = srgb
            ? [context PNGRepresentationOfImage:image
                                         format:kCIFormatRGBA8
                                     colorSpace:srgb
                                        options:@{}]
            : nil;
        if (srgb) CGColorSpaceRelease(srgb);
        CVPixelBufferRelease(frame);
        if (!png.length || png.length > kAgentermMaxPngBytes) {
            FailFrame(out, AGENTERM_DEVICE_STREAM_ENCODE_FAILED,
                      png.length ? @"the encoded PNG exceeds the 64 MiB limit"
                                 : @"the captured frame could not be encoded as PNG");
            return;
        }

        out->png = malloc(png.length);
        if (!out->png) {
            FailFrame(out, AGENTERM_DEVICE_STREAM_ENCODE_FAILED,
                      @"cannot allocate the encoded PNG frame");
            return;
        }
        memcpy(out->png, png.bytes, png.length);
        out->png_len = png.length;
        out->status = AGENTERM_DEVICE_STREAM_OK;
    }
}

void agenterm_device_capture_selected(const char *uid, uint64_t timeout_ms,
                                      AgentermDeviceCaptureFrame *out) {
    if (!out) return;
    memset(out, 0, sizeof(*out));
    out->status = AGENTERM_DEVICE_STREAM_OPEN_FAILED;
    @autoreleasepool {
        @try {
            CaptureSelectedImpl(uid, timeout_ms, out);
        } @catch (NSException *exception) {
            free(out->png);
            out->png = NULL;
            out->png_len = 0;
            FailFrame(out, AGENTERM_DEVICE_STREAM_OPEN_FAILED,
                      [NSString stringWithFormat:@"native device capture failed: %@",
                                                 exception.reason ?: exception.name]);
        }
    }
}

void agenterm_device_capture_frame_free(AgentermDeviceCaptureFrame *out) {
    if (!out) return;
    free(out->png);
    out->png = NULL;
    out->png_len = 0;
}
