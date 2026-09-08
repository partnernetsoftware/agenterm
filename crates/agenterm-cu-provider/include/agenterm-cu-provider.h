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

#ifdef __cplusplus
}
#endif

#endif
