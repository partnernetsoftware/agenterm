/* agenterm-ape.com trampoline: pick the six-cell payload for this host,
 * extract to a private temp dir, spawn+wait, then delete the extract dir.
 * ISA is compile-time per cosmocc slice; OS is runtime.
 */
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <spawn.h>
#include <unistd.h>
#include <signal.h>

#ifndef O_NOFOLLOW
#define O_NOFOLLOW 0
#endif
#ifndef O_DIRECTORY
#define O_DIRECTORY 0
#endif
#ifndef AT_SYMLINK_NOFOLLOW
#define AT_SYMLINK_NOFOLLOW 0
#endif

#ifdef __COSMOPOLITAN__
#include <cosmo.h>
#endif

extern char **environ;

static const char *host_cell(void) {
#ifdef __COSMOPOLITAN__
#if defined(__aarch64__)
    if (IsXnu()) return "osx-aarch64";
    if (IsLinux()) return "lnx-aarch64";
    if (IsWindows()) return "win-aarch64";
#else
    if (IsXnu()) return "osx-x86_64";
    if (IsLinux()) return "lnx-x86_64";
    if (IsWindows()) return "win-x86_64";
#endif
    return NULL;
#else
#if defined(__APPLE__) && defined(__aarch64__)
    return "osx-aarch64";
#elif defined(__APPLE__) && defined(__x86_64__)
    return "osx-x86_64";
#elif defined(__linux__) && defined(__aarch64__)
    return "lnx-aarch64";
#elif defined(__linux__) && defined(__x86_64__)
    return "lnx-x86_64";
#else
    return NULL;
#endif
#endif
}

static int is_windows_cell(const char *cell) {
    return cell && strncmp(cell, "win-", 4) == 0;
}

static void join3(char *out, size_t n, const char *a, const char *b, const char *c) {
    snprintf(out, n, "%s/%s/%s", a, b, c);
}

static int copy_file(const char *src, const char *dst) {
    FILE *in = fopen(src, "rb");
    FILE *out;
    char buf[1 << 16];
    size_t n;
    int err = 0;
    if (!in) return -1;
    out = fopen(dst, "wb");
    if (!out) {
        fclose(in);
        return -1;
    }
    while ((n = fread(buf, 1, sizeof buf, in)) > 0) {
        if (fwrite(buf, 1, n, out) != n) {
            err = 1;
            break;
        }
    }
    if (ferror(in)) err = 1;
    if (fflush(out) != 0) err = 1;
    if (fclose(out) != 0) err = 1;
    fclose(in);
    if (err) {
        unlink(dst);
        return -1;
    }
    return 0;
}

#define EXTRACT_MARK ".agenterm-extract"
#define EXTRACT_MARK_PREFIX "agenterm-ape.com-extract-v1 uid="

static int write_owner_mark(const char *dir) {
    char path[1200];
    char buf[64];
    int fd;
    int n;
    snprintf(path, sizeof path, "%s/%s", dir, EXTRACT_MARK);
    fd = open(path, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW, 0600);
    if (fd < 0) return -1;
    n = snprintf(buf, sizeof buf, EXTRACT_MARK_PREFIX "%ld\n", (long)geteuid());
    if (n < 0 || write(fd, buf, (size_t)n) != n) {
        close(fd);
        unlink(path);
        return -1;
    }
    if (close(fd) != 0) {
        unlink(path);
        return -1;
    }
    return 0;
}

static int is_private_owned_dir(const char *path) {
    struct stat st;
    if (lstat(path, &st) != 0) return 0;
    if (S_ISLNK(st.st_mode) || !S_ISDIR(st.st_mode)) return 0;
    if (st.st_uid != geteuid()) return 0;
    if ((st.st_mode & 0777) != 0700) return 0;
    return 1;
}

static void cleanup_extract(const char *dir, const char *dst, const char *tmpf) {
    int attempt;
    struct stat st;
    char mark[1200];
    int attempts = 50;
    if (!dir || !dir[0]) return;
    snprintf(mark, sizeof mark, "%s/%s", dir, EXTRACT_MARK);
    for (attempt = 0; attempt < attempts; attempt++) {
        if (lstat(dir, &st) != 0) return;
        if (S_ISLNK(st.st_mode)) return;
        if (tmpf && tmpf[0]) unlink(tmpf);
        if (dst && dst[0]) unlink(dst);
        unlink(mark);
        rmdir(dir);
        if (lstat(dir, &st) != 0) return;
        if (S_ISLNK(st.st_mode)) return;
        if (attempt + 1 < attempts) usleep(100000);
    }
}

static int wait_child(pid_t pid, int *st) {
    pid_t w;
    do {
        w = waitpid(pid, st, 0);
    } while (w < 0 && errno == EINTR);
    return w;
}

static int run_payload(const char *dst, char **nargv) {
    pid_t pid;
    int st;
    int rc;

    rc = posix_spawn(&pid, dst, NULL, NULL, nargv, environ);
    if (rc != 0) {
        fprintf(stderr, "agenterm-ape.com: posix_spawn %s: %s\n", dst, strerror(rc));
        return 6;
    }
    if (wait_child(pid, &st) < 0) {
        fprintf(stderr, "agenterm-ape.com: waitpid: %s\n", strerror(errno));
        return -8;
    }
    if (WIFEXITED(st)) return WEXITSTATUS(st);
    if (WIFSIGNALED(st)) return 128 + WTERMSIG(st);
    return 7;
}

int main(int argc, char **argv) {
    const char *cell = host_cell();
    const char *root = getenv("AGENTERM_APE_CELLS");
    const char *leaf;
    char src[1024];
    char tmpl[256];
    char *dir;
    char tmpf[1100];
    char dst[1100];
    char **nargv;
    int i;
    int code;

    if (!cell) {
        fprintf(stderr, "agenterm-ape.com: unknown host cell\n");
        return 2;
    }
    leaf = is_windows_cell(cell) ? "agenterm.exe" : "agenterm";
    if (!root) {
#ifdef __COSMOPOLITAN__
        root = "/zip/cells";
#else
        fprintf(stderr, "agenterm-ape.com: set AGENTERM_APE_CELLS to packed cells dir\n");
        return 2;
#endif
    }

    join3(src, sizeof src, root, cell, leaf);
    if (access(src, F_OK) != 0) {
        fprintf(stderr, "agenterm-ape.com: missing payload %s (%s)\n", src, strerror(errno));
        return 3;
    }

    snprintf(tmpl, sizeof tmpl, "/tmp/agenterm-ape.com.%d.XXXXXX", (int)getpid());
    dir = mkdtemp(tmpl);
    if (!dir) {
        fprintf(stderr, "agenterm-ape.com: mkdtemp: %s\n", strerror(errno));
        return 4;
    }
    if (chmod(dir, 0700) != 0) {
        fprintf(stderr, "agenterm-ape.com: chmod dir: %s\n", strerror(errno));
        rmdir(dir);
        return 4;
    }
    if (write_owner_mark(dir) != 0) {
        fprintf(stderr, "agenterm-ape.com: owner mark: %s\n", strerror(errno));
        rmdir(dir);
        return 4;
    }
    snprintf(tmpf, sizeof tmpf, "%s/.payload.tmp", dir);
    snprintf(dst, sizeof dst, "%s/%s", dir, leaf);
    if (copy_file(src, tmpf) != 0) {
        fprintf(stderr, "agenterm-ape.com: extract %s -> %s failed\n", src, tmpf);
        cleanup_extract(dir, NULL, tmpf);
        return 4;
    }
    if (chmod(tmpf, 0755) != 0) {
        fprintf(stderr, "agenterm-ape.com: chmod payload: %s\n", strerror(errno));
        cleanup_extract(dir, NULL, tmpf);
        return 4;
    }
    if (rename(tmpf, dst) != 0) {
        fprintf(stderr, "agenterm-ape.com: rename: %s\n", strerror(errno));
        cleanup_extract(dir, dst, tmpf);
        return 4;
    }

    nargv = calloc((size_t)argc + 1, sizeof *nargv);
    if (!nargv) {
        cleanup_extract(dir, dst, NULL);
        return 5;
    }
    nargv[0] = dst;
    for (i = 1; i < argc; i++) nargv[i] = argv[i];
    code = run_payload(dst, nargv);
    free(nargv);
    if (code != -8) cleanup_extract(dir, dst, NULL);
    return code < 0 ? 6 : code;
}
