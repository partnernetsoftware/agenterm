#include "process_inspection.h"

#include <errno.h>
#include <libproc.h>
#include <stdlib.h>
#include <string.h>
#include <sys/proc_info.h>

static int32_t native_error(void) {
  return (errno == EPERM || errno == EACCES) ? 2 : 3;
}

static uint32_t bounded_bytes(uint8_t *out, size_t capacity, const char *text,
                              size_t native_capacity) {
  size_t length = 0;
  while (length < native_capacity && text[length] != '\0') length++;
  if (length > capacity) length = capacity;
  if (length > 0) memcpy(out, text, length);
  return (uint32_t)length;
}

int32_t agt_process_fds(uint32_t pid, agt_process_fd *out, size_t capacity,
                        size_t *visited, size_t *written, size_t *read_errors,
                        int32_t *truncated) {
  if (pid == 0 || out == NULL || capacity == 0 || visited == NULL ||
      written == NULL || read_errors == NULL || truncated == NULL) return 1;
  int needed = proc_pidinfo((int)pid, PROC_PIDLISTFDS, 0, NULL, 0);
  if (needed <= 0) return native_error();
  size_t available = (size_t)needed / sizeof(struct proc_fdinfo);
  size_t request = available + 32;
  if (request > capacity + 1) request = capacity + 1;
  struct proc_fdinfo *native = calloc(request, sizeof(*native));
  if (native == NULL) return 3;
  int bytes = proc_pidinfo((int)pid, PROC_PIDLISTFDS, 0, native,
                           (int)(request * sizeof(*native)));
  if (bytes < 0) { free(native); return native_error(); }
  available = (size_t)bytes / sizeof(*native);
  *truncated = available > capacity;
  *visited = available < capacity ? available : capacity;
  *written = 0;
  *read_errors = 0;
  for (size_t index = 0; index < *visited; index++) {
    agt_process_fd row;
    memset(&row, 0, sizeof(row));
    row.descriptor = native[index].proc_fd;
    row.kind = native[index].proc_fdtype;
    if (row.kind == PROX_FDTYPE_VNODE) {
      struct vnode_fdinfowithpath vnode;
      memset(&vnode, 0, sizeof(vnode));
      int got = proc_pidfdinfo((int)pid, row.descriptor,
                               PROC_PIDFDVNODEPATHINFO, &vnode,
                               (int)sizeof(vnode));
      if (got == (int)sizeof(vnode)) {
        row.has_vnode = 1;
        row.open_flags = vnode.pfi.fi_openflags;
        row.status_flags = vnode.pfi.fi_status;
        row.offset_bytes = vnode.pfi.fi_offset;
        row.file_type = vnode.pfi.fi_type;
        row.guard_flags = vnode.pfi.fi_guardflags;
        row.target_len = bounded_bytes(row.target, sizeof(row.target),
                                       vnode.pvip.vip_path, MAXPATHLEN);
      } else {
        (*read_errors)++;
      }
    }
    out[(*written)++] = row;
  }
  free(native);
  return 0;
}

int32_t agt_process_regions(uint32_t pid, agt_process_region *out,
                            size_t capacity, size_t *visited,
                            size_t *written, int32_t *truncated) {
  if (pid == 0 || out == NULL || capacity == 0 || visited == NULL ||
      written == NULL || truncated == NULL) return 1;
  uint64_t cursor = 0;
  *visited = 0;
  *written = 0;
  *truncated = 0;
  for (;;) {
    struct proc_regionwithpathinfo native;
    memset(&native, 0, sizeof(native));
    int bytes = proc_pidinfo((int)pid, PROC_PIDREGIONPATHINFO, cursor,
                             &native, (int)sizeof(native));
    if (bytes <= 0) {
      if (*visited == 0 && (errno == EPERM || errno == EACCES)) return 2;
      break;
    }
    if (bytes != (int)sizeof(native) || native.prp_prinfo.pri_size == 0)
      return 4;
    if (*written == capacity) { *truncated = 1; break; }
    agt_process_region row;
    memset(&row, 0, sizeof(row));
    const struct proc_regioninfo *info = &native.prp_prinfo;
    row.start_address = info->pri_address;
    row.size_bytes = info->pri_size;
    row.offset_bytes = info->pri_offset;
    row.protection = info->pri_protection;
    row.max_protection = info->pri_max_protection;
    row.flags = info->pri_flags;
    row.sharing = info->pri_share_mode;
    row.resident_pages = info->pri_pages_resident;
    row.private_resident_pages = info->pri_private_pages_resident;
    row.shared_resident_pages = info->pri_shared_pages_resident;
    row.swapped_pages = info->pri_pages_swapped_out;
    row.dirtied_pages = info->pri_pages_dirtied;
    row.user_tag = info->pri_user_tag;
    row.depth = info->pri_depth;
    row.path_len = bounded_bytes(row.path, sizeof(row.path),
                                 native.prp_vip.vip_path, MAXPATHLEN);
    out[(*written)++] = row;
    (*visited)++;
    uint64_t next = info->pri_address + info->pri_size;
    if (next <= cursor || next <= info->pri_address) break;
    cursor = next;
  }
  return 0;
}

int32_t agt_process_threads(uint32_t pid, agt_process_thread *out,
                            size_t capacity, size_t *visited,
                            size_t *written, size_t *read_errors,
                            int32_t *truncated) {
  if (pid == 0 || out == NULL || capacity == 0 || visited == NULL ||
      written == NULL || read_errors == NULL || truncated == NULL) return 1;
  uint64_t *identifiers = calloc(capacity + 1, sizeof(uint64_t));
  if (identifiers == NULL) return 3;
  int bytes = proc_pidinfo((int)pid, PROC_PIDLISTTHREADS, 0, identifiers,
                           (int)((capacity + 1) * sizeof(uint64_t)));
  if (bytes <= 0) { free(identifiers); return native_error(); }
  size_t listed = (size_t)bytes / sizeof(uint64_t);
  *truncated = listed > capacity;
  *visited = listed < capacity ? listed : capacity;
  *written = 0;
  *read_errors = 0;
  for (size_t index = 0; index < *visited; index++) {
    struct proc_threadinfo native;
    memset(&native, 0, sizeof(native));
    int bytes = proc_pidinfo((int)pid, PROC_PIDTHREADINFO, identifiers[index],
                             &native, (int)sizeof(native));
    if (bytes != (int)sizeof(native)) { (*read_errors)++; continue; }
    agt_process_thread row;
    memset(&row, 0, sizeof(row));
    row.id = identifiers[index];
    row.user_time = native.pth_user_time;
    row.system_time = native.pth_system_time;
    row.cpu_usage = native.pth_cpu_usage;
    row.policy = native.pth_policy;
    row.run_state = native.pth_run_state;
    row.flags = native.pth_flags;
    row.sleep_seconds = native.pth_sleep_time;
    row.current_priority = native.pth_curpri;
    row.priority = native.pth_priority;
    row.max_priority = native.pth_maxpriority;
    row.name_len = bounded_bytes(row.name, sizeof(row.name), native.pth_name,
                                 MAXTHREADNAMESIZE);
    out[(*written)++] = row;
  }
  free(identifiers);
  return 0;
}
