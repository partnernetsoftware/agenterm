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
    AGENTERM_CU_PROVIDER_PANICKED = 6
};

uint32_t agenterm_cu_provider_abi_version(void);

int32_t agenterm_cu_provider_call(const uint8_t *request,
                                  size_t request_len,
                                  uint8_t *reply,
                                  size_t reply_capacity,
                                  size_t *reply_len);

#ifdef __cplusplus
}
#endif

#endif
