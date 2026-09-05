#ifndef AGENTERM_PLATFORM_PROCESS_INSPECTION_H
#define AGENTERM_PLATFORM_PROCESS_INSPECTION_H

#include <stddef.h>
#include <stdint.h>

#define AGT_PROCESS_PATH_MAX 1024
#define AGT_PROCESS_THREAD_NAME_MAX 64
#define AGT_PROCESS_ENDPOINT_MAX 1024

typedef struct {
  int32_t descriptor;
  uint32_t kind;
  uint32_t has_vnode;
  uint32_t open_flags;
  uint32_t status_flags;
  int64_t offset_bytes;
  uint32_t file_type;
  uint32_t guard_flags;
  uint32_t target_len;
  uint8_t target[AGT_PROCESS_PATH_MAX];
} agt_process_fd;

typedef struct {
  uint64_t start_address;
  uint64_t size_bytes;
  uint64_t offset_bytes;
  uint32_t protection;
  uint32_t max_protection;
  uint32_t flags;
  uint32_t sharing;
  uint32_t resident_pages;
  uint32_t private_resident_pages;
  uint32_t shared_resident_pages;
  uint32_t swapped_pages;
  uint32_t dirtied_pages;
  uint32_t user_tag;
  uint32_t depth;
  uint32_t path_len;
  uint8_t path[AGT_PROCESS_PATH_MAX];
} agt_process_region;

typedef struct {
  uint64_t id;
  uint64_t user_time;
  uint64_t system_time;
  int32_t cpu_usage;
  int32_t policy;
  int32_t run_state;
  int32_t flags;
  int32_t sleep_seconds;
  int32_t current_priority;
  int32_t priority;
  int32_t max_priority;
  uint32_t name_len;
  uint8_t name[AGT_PROCESS_THREAD_NAME_MAX];
} agt_process_thread;

typedef struct {
  int32_t descriptor;
  int32_t family;
  int32_t socket_type;
  int32_t protocol;
  int32_t tcp_state;
  uint32_t generic_state;
  uint32_t local_len;
  uint32_t remote_len;
  uint8_t local[AGT_PROCESS_ENDPOINT_MAX];
  uint8_t remote[AGT_PROCESS_ENDPOINT_MAX];
} agt_process_socket;

/* 0 success, 1 invalid input, 2 denied, 3 native failure, 4 malformed. */
int32_t agt_process_fds(uint32_t pid, agt_process_fd *out, size_t capacity,
                        size_t *visited, size_t *written, size_t *read_errors,
                        int32_t *truncated);
int32_t agt_process_regions(uint32_t pid, agt_process_region *out,
                            size_t capacity, size_t *visited,
                            size_t *written, int32_t *truncated);
int32_t agt_process_threads(uint32_t pid, agt_process_thread *out,
                            size_t capacity, size_t *visited,
                            size_t *written, size_t *read_errors,
                            int32_t *truncated);
int32_t agt_process_sockets(uint32_t pid, agt_process_socket *out,
                            size_t capacity, size_t *visited,
                            size_t *written, size_t *read_errors,
                            int32_t *truncated);

#endif
