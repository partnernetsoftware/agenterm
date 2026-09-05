#include "process_inspection.h"

#include <errno.h>
#include <libproc.h>
#include <arpa/inet.h>
#include <netinet/in.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
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

static uint32_t endpoint_bytes(uint8_t *out, size_t capacity,
                               const struct in_sockinfo *info, int local) {
  char address[INET6_ADDRSTRLEN];
  char endpoint[INET6_ADDRSTRLEN + 16];
  const void *raw = NULL;
  int family = AF_UNSPEC;
  if ((info->insi_vflag & INI_IPV4) != 0) {
    family = AF_INET;
    raw = local ? (const void *)&info->insi_laddr.ina_46.i46a_addr4
                : (const void *)&info->insi_faddr.ina_46.i46a_addr4;
  } else if ((info->insi_vflag & INI_IPV6) != 0) {
    family = AF_INET6;
    raw = local ? (const void *)&info->insi_laddr.ina_6
                : (const void *)&info->insi_faddr.ina_6;
  }
  if (raw == NULL || inet_ntop(family, raw, address, sizeof(address)) == NULL)
    return 0;
  unsigned port = ntohs((uint16_t)(local ? info->insi_lport : info->insi_fport));
  int length = family == AF_INET6
                   ? snprintf(endpoint, sizeof(endpoint), "[%s]:%u", address, port)
                   : snprintf(endpoint, sizeof(endpoint), "%s:%u", address, port);
  if (length <= 0) return 0;
  size_t bounded = (size_t)length;
  if (bounded >= sizeof(endpoint)) bounded = sizeof(endpoint) - 1;
  if (bounded > capacity) bounded = capacity;
  memcpy(out, endpoint, bounded);
  return (uint32_t)bounded;
}

static uint32_t unix_endpoint_bytes(uint8_t *out, size_t capacity,
                                    const struct sockaddr_un *address) {
  size_t offset = offsetof(struct sockaddr_un, sun_path);
  if (address->sun_len <= offset) return 0;
  size_t length = (size_t)address->sun_len - offset;
  if (length > sizeof(address->sun_path)) length = sizeof(address->sun_path);
  if (length > 0 && address->sun_path[length - 1] == '\0') length--;
  if (length > capacity) length = capacity;
  if (length > 0) memcpy(out, address->sun_path, length);
  return (uint32_t)length;
}

int32_t agt_process_sockets(uint32_t pid, agt_process_socket *out,
                            size_t capacity, size_t *visited,
                            size_t *written, size_t *read_errors,
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
  if ((size_t)bytes % sizeof(*native) != 0) { free(native); return 4; }
  available = (size_t)bytes / sizeof(*native);
  *truncated = available > capacity;
  *visited = available < capacity ? available : capacity;
  *written = 0;
  *read_errors = 0;
  for (size_t index = 0; index < *visited; index++) {
    if (native[index].proc_fdtype != PROX_FDTYPE_SOCKET) continue;
    struct socket_fdinfo info;
    memset(&info, 0, sizeof(info));
    int got = proc_pidfdinfo((int)pid, native[index].proc_fd,
                             PROC_PIDFDSOCKETINFO, &info, (int)sizeof(info));
    if (got != (int)sizeof(info)) { (*read_errors)++; continue; }
    agt_process_socket row;
    memset(&row, 0, sizeof(row));
    row.descriptor = native[index].proc_fd;
    row.family = info.psi.soi_family;
    row.socket_type = info.psi.soi_type;
    row.protocol = info.psi.soi_protocol;
    row.tcp_state = -1;
    row.generic_state = (uint16_t)info.psi.soi_state;
    if (info.psi.soi_kind == SOCKINFO_TCP) {
      const struct tcp_sockinfo *tcp = &info.psi.soi_proto.pri_tcp;
      row.tcp_state = tcp->tcpsi_state;
      row.local_len = endpoint_bytes(row.local, sizeof(row.local),
                                     &tcp->tcpsi_ini, 1);
      row.remote_len = endpoint_bytes(row.remote, sizeof(row.remote),
                                      &tcp->tcpsi_ini, 0);
    } else if (info.psi.soi_kind == SOCKINFO_IN) {
      const struct in_sockinfo *inet = &info.psi.soi_proto.pri_in;
      row.local_len = endpoint_bytes(row.local, sizeof(row.local), inet, 1);
      row.remote_len = endpoint_bytes(row.remote, sizeof(row.remote), inet, 0);
    } else if (info.psi.soi_kind == SOCKINFO_UN) {
      const struct un_sockinfo *unix_info = &info.psi.soi_proto.pri_un;
      row.local_len = unix_endpoint_bytes(row.local, sizeof(row.local),
                                          &unix_info->unsi_addr.ua_sun);
      row.remote_len = unix_endpoint_bytes(row.remote, sizeof(row.remote),
                                           &unix_info->unsi_caddr.ua_sun);
    }
    out[(*written)++] = row;
  }
  free(native);
  return 0;
}
