#import <Foundation/Foundation.h>

#include "usbmux_probe.h"

#include <errno.h>
#include <stdint.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/time.h>
#include <sys/un.h>
#include <unistd.h>

static const char *kUsbmuxSocket = "/var/run/usbmuxd";

typedef struct {
    uint32_t length;
    uint32_t version;
    uint32_t message;
    uint32_t tag;
} MuxHeader;

static void CopyError(char *slot, size_t capacity, NSString *message) {
    if (!slot || capacity == 0) return;
    slot[0] = '\0';
    if (message) {
        [message getCString:slot maxLength:capacity encoding:NSUTF8StringEncoding];
    }
}

static BOOL WriteFully(int fd, const void *bytes, size_t length) {
    const uint8_t *cursor = bytes;
    while (length > 0) {
        ssize_t written = send(fd, cursor, length, MSG_NOSIGNAL);
        if (written < 0 && errno == EINTR) continue;
        if (written <= 0) return NO;
        cursor += (size_t)written;
        length -= (size_t)written;
    }
    return YES;
}

static BOOL ReadFully(int fd, void *bytes, size_t length) {
    uint8_t *cursor = bytes;
    while (length > 0) {
        ssize_t read_count = read(fd, cursor, length);
        if (read_count < 0 && errno == EINTR) continue;
        if (read_count <= 0) return NO;
        cursor += (size_t)read_count;
        length -= (size_t)read_count;
    }
    return YES;
}

// One bounded request and reply per connection. The operation is observation
// only; no PairDevice, DeletePairRecord, Connect or Listen request is sent.
static NSDictionary *MuxExchange(NSDictionary *request, NSString **failure) {
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0) {
        if (failure) *failure = [NSString stringWithFormat:@"cannot open usbmuxd socket: %s", strerror(errno)];
        return nil;
    }

    struct timeval timeout = {.tv_sec = 1, .tv_usec = 0};
    (void)setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout));
    (void)setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &timeout, sizeof(timeout));

    struct sockaddr_un address = {0};
    address.sun_family = AF_UNIX;
    strlcpy(address.sun_path, kUsbmuxSocket, sizeof(address.sun_path));
    if (connect(fd, (struct sockaddr *)&address, sizeof(address)) != 0) {
        int saved_errno = errno;
        close(fd);
        if (failure) *failure = [NSString stringWithFormat:@"cannot connect to usbmuxd: %s", strerror(saved_errno)];
        return nil;
    }

    NSMutableDictionary *message = [request mutableCopy];
    message[@"ClientVersionString"] = @"agenterm-platform";
    message[@"ProgName"] = @"agenterm-platform";
    message[@"kLibUSBMuxVersion"] = @3;

    NSError *serialization_error = nil;
    NSData *payload =
        [NSPropertyListSerialization dataWithPropertyList:message
                                                   format:NSPropertyListXMLFormat_v1_0
                                                  options:0
                                                    error:&serialization_error];
    if (!payload || payload.length > UINT32_MAX - sizeof(MuxHeader)) {
        close(fd);
        if (failure) {
            *failure = serialization_error.localizedDescription ?: @"cannot encode usbmuxd request";
        }
        return nil;
    }

    MuxHeader header = {(uint32_t)(sizeof(MuxHeader) + payload.length), 1, 8, 1};
    if (!WriteFully(fd, &header, sizeof(header)) ||
        !WriteFully(fd, payload.bytes, payload.length)) {
        int saved_errno = errno;
        close(fd);
        if (failure) *failure = [NSString stringWithFormat:@"cannot write usbmuxd request: %s", strerror(saved_errno)];
        return nil;
    }

    MuxHeader reply = {0};
    if (!ReadFully(fd, &reply, sizeof(reply)) || reply.length < sizeof(reply) ||
        reply.length > (1u << 22) || reply.version != 1 || reply.message != 8) {
        int saved_errno = errno;
        close(fd);
        if (failure) {
            *failure = saved_errno
                ? [NSString stringWithFormat:@"cannot read usbmuxd reply: %s", strerror(saved_errno)]
                : @"usbmuxd returned an invalid reply header";
        }
        return nil;
    }

    size_t body_length = reply.length - sizeof(reply);
    NSMutableData *body = [NSMutableData dataWithLength:body_length];
    if (!ReadFully(fd, body.mutableBytes, body_length)) {
        int saved_errno = errno;
        close(fd);
        if (failure) *failure = [NSString stringWithFormat:@"cannot read usbmuxd reply body: %s", strerror(saved_errno)];
        return nil;
    }
    close(fd);

    NSError *parse_error = nil;
    id parsed = [NSPropertyListSerialization propertyListWithData:body
                                                          options:0
                                                           format:NULL
                                                            error:&parse_error];
    if (![parsed isKindOfClass:NSDictionary.class]) {
        if (failure) *failure = parse_error.localizedDescription ?: @"usbmuxd returned a non-dictionary reply";
        return nil;
    }
    return parsed;
}

AgentermUsbmuxObservation agenterm_usbmux_observe(void) {
    @autoreleasepool {
        AgentermUsbmuxObservation result = {0};
        NSString *failure = nil;
        NSDictionary *reply = MuxExchange(@{@"MessageType" : @"ListDevices"}, &failure);
        NSArray *device_list = reply[@"DeviceList"];
        if (!reply || ![device_list isKindOfClass:NSArray.class]) {
            CopyError(result.message, sizeof(result.message), failure ?: @"usbmuxd returned no device inventory");
            return result;
        }

        for (id value in device_list) {
            if (![value isKindOfClass:NSDictionary.class]) continue;
            id properties = ((NSDictionary *)value)[@"Properties"];
            if (![properties isKindOfClass:NSDictionary.class]) continue;
            id serial_value = ((NSDictionary *)properties)[@"SerialNumber"];
            if (![serial_value isKindOfClass:NSString.class] || ![(NSString *)serial_value length]) {
                continue;
            }

            result.connected_devices++;
            failure = nil;
            NSDictionary *pair_reply = MuxExchange(
                @{@"MessageType" : @"ReadPairRecord", @"PairRecordID" : serial_value},
                &failure);
            if (!pair_reply) {
                CopyError(result.message, sizeof(result.message), failure ?: @"usbmuxd pair-record query failed");
                result.connected_devices = 0;
                result.paired_devices = 0;
                return result;
            }

            id record = pair_reply[@"PairRecordData"];
            if ([record isKindOfClass:NSData.class] && [(NSData *)record length] > 0) {
                result.paired_devices++;
                continue;
            }
            id status = pair_reply[@"Number"] ?: pair_reply[@"Result"];
            if (![status isKindOfClass:NSNumber.class]) {
                CopyError(result.message, sizeof(result.message), @"usbmuxd returned an invalid pair-record reply");
                result.connected_devices = 0;
                result.paired_devices = 0;
                return result;
            }
        }

        result.available = 1;
        return result;
    }
}
