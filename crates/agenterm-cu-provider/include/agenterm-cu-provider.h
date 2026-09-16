#ifndef AGENTERM_CU_PROVIDER_H
#define AGENTERM_CU_PROVIDER_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define AGENTERM_CU_PROVIDER_ABI_VERSION UINT32_C(1)
#define AGENTERM_CU_PROVIDER_MAX_REQUEST_BYTES ((size_t)1048576)
#define AGENTERM_CU_PROVIDER_MAX_REPLY_BYTES ((size_t)4194304)
#define AGENTERM_CU_PROCESS_MAIN_ABI_VERSION UINT32_C(1)
#define AGENTERM_CU_PROCESS_MAIN_MAX_ARGV_COUNT ((size_t)4096)
#define AGENTERM_CU_PROCESS_MAIN_MAX_ARGV_BYTES ((size_t)1048576)
#define AGENTERM_CU_PROCESS_MAIN_MAX_STDOUT_BYTES ((size_t)4194304)
#define AGENTERM_CU_PROCESS_MAIN_MAX_STDERR_BYTES ((size_t)1048576)

enum agenterm_cu_provider_status {
    AGENTERM_CU_PROVIDER_OK = 0,
    AGENTERM_CU_PROVIDER_INVALID_POINTER = 1,
    AGENTERM_CU_PROVIDER_REQUEST_TOO_LARGE = 2,
    AGENTERM_CU_PROVIDER_REQUEST_NOT_UTF8 = 3,
    AGENTERM_CU_PROVIDER_REPLY_TOO_LARGE = 4,
    AGENTERM_CU_PROVIDER_SERIALIZE_FAILED = 5,
    AGENTERM_CU_PROVIDER_PANICKED = 6,
    AGENTERM_CU_PROVIDER_INVALID_CANCEL = 7
};

enum agenterm_cu_process_main_status {
    AGENTERM_CU_PROCESS_MAIN_OK = 0,
    AGENTERM_CU_PROCESS_MAIN_INVALID_POINTER = 1,
    AGENTERM_CU_PROCESS_MAIN_BAD_REQUEST = 2,
    AGENTERM_CU_PROCESS_MAIN_ARGV_TOO_LARGE = 3,
    AGENTERM_CU_PROCESS_MAIN_ARGV_NOT_UTF8 = 4,
    AGENTERM_CU_PROCESS_MAIN_OUTPUT_TOO_LARGE = 5,
    AGENTERM_CU_PROCESS_MAIN_SERIALIZE_FAILED = 6,
    AGENTERM_CU_PROCESS_MAIN_PROVIDER_PANICKED = 7,
    AGENTERM_CU_PROCESS_MAIN_ENTRY_MODE_UNIMPLEMENTED = 8
};

struct agenterm_cu_byte_span_v1 {
    const uint8_t *data;
    size_t len;
};

struct agenterm_cu_process_main_request_v1 {
    uint32_t abi_version;
    uint32_t struct_size;
    size_t argc;
    const struct agenterm_cu_byte_span_v1 *argv;
};

struct agenterm_cu_process_main_result_v1 {
    uint32_t abi_version;
    uint32_t struct_size;
    uint32_t entry_mode;
    int32_t exit_code;
    size_t stdout_len;
    size_t stderr_len;
};

/*
 * Process-main argv spans contain raw UTF-8 bytes without a trailing NUL;
 * embedded NUL bytes are rejected. The provider classifies entry_mode and the
 * launcher must require it to match its boundary prediction before publishing
 * buffered output. Only the thin launcher consumes this ABI; embedded hosts
 * continue to use agenterm_cu_provider_call[_v2].
 */

typedef uint8_t (*agenterm_cu_is_cancelled_fn)(const void *context);

struct agenterm_cu_cancel_v1 {
    size_t struct_size;
    uint32_t version;
    const void *context;
    agenterm_cu_is_cancelled_fn is_cancelled;
};

uint32_t agenterm_cu_provider_abi_version(void);

int32_t agenterm_cu_provider_call(const uint8_t *request,
                                  size_t request_len,
                                  uint8_t *reply,
                                  size_t reply_capacity,
                                  size_t *reply_len);

int32_t agenterm_cu_provider_call_v2(const uint8_t *request,
                                     size_t request_len,
                                     uint8_t *reply,
                                     size_t reply_capacity,
                                     size_t *reply_len,
                                     const struct agenterm_cu_cancel_v1 *cancel);

uint32_t agenterm_cu_process_main_abi_version(void);

int32_t agenterm_cu_process_main_v1(
    const struct agenterm_cu_process_main_request_v1 *request,
    uint8_t *stdout_bytes,
    size_t stdout_capacity,
    uint8_t *stderr_bytes,
    size_t stderr_capacity,
    struct agenterm_cu_process_main_result_v1 *result);

#ifdef __cplusplus
}
#endif

#endif
