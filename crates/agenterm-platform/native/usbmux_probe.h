#ifndef AGENTERM_PLATFORM_USBMUX_PROBE_H
#define AGENTERM_PLATFORM_USBMUX_PROBE_H

#include <stddef.h>

typedef struct {
    int available;
    size_t connected_devices;
    size_t paired_devices;
    char message[512];
} AgentermUsbmuxObservation;

// Perform only usbmuxd's read-only ListDevices and ReadPairRecord requests.
// `available == 0` means the probe itself did not produce reliable inventory;
// it must never be interpreted as a zero-device result.
AgentermUsbmuxObservation agenterm_usbmux_observe(void);

#endif
