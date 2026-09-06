#include <errno.h>
#include <libproc.h>
#include <mach/mach.h>
#include <mach/task_policy.h>
#include <signal.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/proc_info.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

static int read_flags(pid_t pid, uint32_t *flags) {
  struct proc_bsdinfo info;
  memset(&info, 0, sizeof(info));
  const int count = proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, sizeof(info));
  if (count != (int)sizeof(info)) return errno == 0 ? EIO : errno;
  *flags = info.pbi_flags;
  return 0;
}

static pid_t spawn_blocked_child(void) {
  const pid_t pid = fork();
  if (pid < 0) return -1;
  if (pid == 0) {
    for (;;) pause();
  }
  return pid;
}

static void stop_child(pid_t pid) {
  if (pid <= 0) return;
  (void)kill(pid, SIGKILL);
  while (waitpid(pid, NULL, 0) < 0 && errno == EINTR) {}
}

int main(void) {
  int result = 1;
  pid_t target = -1;
  pid_t sibling = -1;
  task_t task = MACH_PORT_NULL;
  uint32_t target_before = 0;
  uint32_t target_background = 0;
  uint32_t target_restored = 0;
  uint32_t sibling_before = 0;
  uint32_t sibling_after = 0;
  bool sibling_after_read = false;
  kern_return_t acquire = KERN_FAILURE;
  kern_return_t get_before = KERN_FAILURE;
  kern_return_t set_background = KERN_FAILURE;
  kern_return_t get_background = KERN_FAILURE;
  kern_return_t restore = KERN_FAILURE;
  kern_return_t get_restored = KERN_FAILURE;
  kern_return_t get_after_exit = KERN_FAILURE;
  bool target_reaped = false;

  target = spawn_blocked_child();
  sibling = spawn_blocked_child();
  if (target <= 0 || sibling <= 0) goto done;
  if (read_flags(target, &target_before) != 0 ||
      read_flags(sibling, &sibling_before) != 0) goto done;

  acquire = task_for_pid(mach_task_self(), target, &task);
  if (acquire != KERN_SUCCESS || task == MACH_PORT_NULL) goto done;

  task_category_policy_data_t policy;
  mach_msg_type_number_t count = TASK_CATEGORY_POLICY_COUNT;
  boolean_t get_default = false;
  memset(&policy, 0, sizeof(policy));
  get_before = task_policy_get(task, TASK_CATEGORY_POLICY,
      (task_policy_t)&policy, &count, &get_default);
  if (get_before != KERN_SUCCESS) goto done;
  const task_role_t original_role = policy.role;

  policy.role = TASK_DARWINBG_APPLICATION;
  set_background = task_policy_set(task, TASK_CATEGORY_POLICY,
      (task_policy_t)&policy, TASK_CATEGORY_POLICY_COUNT);
  if (set_background != KERN_SUCCESS) goto done;
  count = TASK_CATEGORY_POLICY_COUNT;
  get_default = false;
  get_background = task_policy_get(task, TASK_CATEGORY_POLICY,
      (task_policy_t)&policy, &count, &get_default);
  if (get_background != KERN_SUCCESS || read_flags(target, &target_background) != 0)
    goto done;

  policy.role = original_role;
  restore = task_policy_set(task, TASK_CATEGORY_POLICY,
      (task_policy_t)&policy, TASK_CATEGORY_POLICY_COUNT);
  if (restore != KERN_SUCCESS) goto done;
  count = TASK_CATEGORY_POLICY_COUNT;
  get_default = false;
  get_restored = task_policy_get(task, TASK_CATEGORY_POLICY,
      (task_policy_t)&policy, &count, &get_default);
  if (get_restored != KERN_SUCCESS || read_flags(target, &target_restored) != 0 ||
      read_flags(sibling, &sibling_after) != 0) goto done;
  sibling_after_read = true;

  stop_child(target);
  target_reaped = true;
  count = TASK_CATEGORY_POLICY_COUNT;
  get_default = false;
  get_after_exit = task_policy_get(task, TASK_CATEGORY_POLICY,
      (task_policy_t)&policy, &count, &get_default);
  result = 0;

done:
  if (sibling > 0 && !sibling_after_read &&
      read_flags(sibling, &sibling_after) == 0) {
    sibling_after_read = true;
  }
  printf("{\"target_pid\":%d,\"sibling_pid\":%d,"
         "\"task_for_pid\":%d,\"get_before\":%d,"
         "\"set_background\":%d,\"get_background\":%d,"
         "\"restore\":%d,\"get_restored\":%d,"
         "\"get_after_exit\":%d,\"target_flags_before\":%u,"
         "\"target_flags_background\":%u,\"target_flags_restored\":%u,"
         "\"sibling_flags_before\":%u,\"sibling_flags_after\":%u,"
         "\"sibling_after_read\":%s}\n",
         target, sibling, acquire, get_before, set_background, get_background,
         restore, get_restored, get_after_exit, target_before,
         target_background, target_restored, sibling_before, sibling_after,
         sibling_after_read ? "true" : "false");
  if (task != MACH_PORT_NULL) mach_port_deallocate(mach_task_self(), task);
  if (!target_reaped) stop_child(target);
  stop_child(sibling);
  return result;
}
