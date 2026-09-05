#ifndef AGENTERM_PLATFORM_DEVICE_CAPTURE_H
#define AGENTERM_PLATFORM_DEVICE_CAPTURE_H

#include <stddef.h>
#include <stdint.h>

#define AGENTERM_DEVICE_CAPTURE_TEXT 512
#define AGENTERM_DEVICE_CAPTURE_NAME 256
#define AGENTERM_DEVICE_CAPTURE_UID 512

enum AgentermCameraAuthorization {
    AGENTERM_CAMERA_AUTHORIZED = 0,
    AGENTERM_CAMERA_DENIED = 1,
    AGENTERM_CAMERA_RESTRICTED = 2,
    AGENTERM_CAMERA_NOT_DETERMINED = 3,
};

enum AgentermObservationStatus {
    AGENTERM_OBSERVATION_INVENTORY = 0,
    AGENTERM_OBSERVATION_FAILED = 1,
};

typedef struct {
    char name[AGENTERM_DEVICE_CAPTURE_NAME];
    char uid[AGENTERM_DEVICE_CAPTURE_UID];
} AgentermDeviceCaptureSource;

typedef struct {
    int32_t camera_authorization;
    int32_t usbmux_status;
    size_t connected_devices;
    size_t paired_devices;
    char usbmux_message[AGENTERM_DEVICE_CAPTURE_TEXT];
    int32_t dal_status;
    AgentermDeviceCaptureSource *sources;
    size_t source_count;
    char dal_message[AGENTERM_DEVICE_CAPTURE_TEXT];
} AgentermDeviceCaptureEvidence;

// Observe Camera authorization, usbmux inventory and DAL inventory without
// requesting Camera access or opening a stream. Always initializes `out`.
void agenterm_device_capture_observe(AgentermDeviceCaptureEvidence *out);
void agenterm_device_capture_evidence_free(AgentermDeviceCaptureEvidence *out);

enum AgentermDeviceStreamStatus {
    AGENTERM_DEVICE_STREAM_OK = 0,
    AGENTERM_DEVICE_STREAM_OPEN_FAILED = 1,
    AGENTERM_DEVICE_STREAM_INPUT_REFUSED = 2,
    AGENTERM_DEVICE_STREAM_OUTPUT_REFUSED = 3,
    AGENTERM_DEVICE_STREAM_START_FAILED = 4,
    AGENTERM_DEVICE_STREAM_FRAME_TIMEOUT = 5,
    AGENTERM_DEVICE_STREAM_EXPLICIT_LOCKED = 6,
    AGENTERM_DEVICE_STREAM_EXPLICIT_NOT_TRUSTED = 7,
    AGENTERM_DEVICE_STREAM_ENCODE_FAILED = 8,
};

typedef struct {
    uint8_t *png;
    size_t png_len;
    uint32_t width;
    uint32_t height;
    int32_t status;
    char message[AGENTERM_DEVICE_CAPTURE_TEXT];
} AgentermDeviceCaptureFrame;

// Capture one frame from the exact DAL uid selected by the shared Rust
// classifier. This function never requests Camera access. A silent stream is
// FRAME_TIMEOUT; it is not evidence that the device is locked or untrusted.
void agenterm_device_capture_selected(const char *uid, uint64_t timeout_ms,
                                      AgentermDeviceCaptureFrame *out);
void agenterm_device_capture_frame_free(AgentermDeviceCaptureFrame *out);

#endif
