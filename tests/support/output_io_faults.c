// Darwin interposition forces filesystem races at the syscall boundary of the real CLI.
#include <errno.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static int published;
static int stage_fd = -1;
static int selected(const char *value) {
    const char *requested = getenv("OUTPUT_FAULT");
    return requested && !strcmp(requested, value);
}
static int intercepted_openat(int dir, const char *name, int flags, ...) {
    mode_t mode = 0;
    if (flags & O_CREAT) {
        va_list args;
        va_start(args, flags);
        mode = va_arg(args, int);
        va_end(args);
    }
    if (!strncmp(name, ".cuke-dedup-", 12)) {
        if (selected("stage-open-remove")) {
            if (unlinkat(dir, name, AT_REMOVEDIR)) _exit(88);
            errno = ENOENT;
            return -1;
        }
        if (selected("stage-open-error")) {
            errno = EACCES;
            return -1;
        }
        if (selected("stage-open-swap") || selected("stage-identity-swap")) {
            if (renameat(dir, name, dir, "saved-stage") || mkdirat(dir, name, 0700)) _exit(88);
            if (selected("stage-open-swap")) {
                errno = EACCES;
                return -1;
            }
        }
        if (selected("stage-permissions") && fchmodat(dir, name, 0777, 0)) _exit(88);
    }
    if (selected("leaf-type") && !strcmp(name, "baseline.json") && (flags & O_RDWR)) {
        if (renameat(dir, name, dir, "saved-baseline") || mkfifoat(dir, name, 0600)) _exit(88);
    }
    int result = openat(dir, name, flags, mode);
    if (!strncmp(name, ".cuke-dedup-", 12)) stage_fd = result;
    return result;
}
static int intercepted_renameat(int from, const char *old, int to, const char *name) {
    if (!strcmp(old, "payload") && !strcmp(name, "cuke-dedup.json") && selected("publish-link")) {
        const char *external = getenv("OUTPUT_SENTINEL");
        if (!external || unlinkat(from, old, 0) || symlinkat(external, from, old)) _exit(88);
    }
    int result = renameat(from, old, to, name);
    if (!result && !strcmp(name, "cuke-dedup.json")) {
        published = 1;
        if (selected("publish-remove") && unlinkat(to, name, 0)) _exit(88);
    }
    return result;
}
static int intercepted_fstatat(int dir, const char *name, struct stat *value, int flags) {
    if (selected("stage-inspect-error") && !strncmp(name, ".cuke-dedup-", 12)) {
        if (unlinkat(dir, name, AT_REMOVEDIR)) _exit(88);
        errno = ENOENT;
        return -1;
    }
    if (published && selected("stat-error") && !strcmp(name, "cuke-dedup.json")) {
        errno = EIO;
        return -1;
    }
    int result = fstatat(dir, name, value, flags);
    if (!result && selected("fixed-mode") && !strncmp(name, ".cuke-dedup-", 12))
        value->st_mode = (value->st_mode & S_IFMT) | 0777;
    return result;
}
static int intercepted_fstat(int fd, struct stat *value) {
    if (selected("stage-metadata-error") && fd == stage_fd) {
        errno = EIO;
        return -1;
    }
    int result = fstat(fd, value);
    if (!result && selected("fixed-mode") && fd == stage_fd)
        value->st_mode = (value->st_mode & S_IFMT) | 0777;
    return result;
}
#define INTERPOSE(new_function, old_function)     __attribute__((used)) static struct { const void *replacement; const void *original; } binding_##old_function     __attribute__((section("__DATA,__interpose"))) = { (const void *)new_function, (const void *)old_function }
INTERPOSE(intercepted_openat, openat);
INTERPOSE(intercepted_renameat, renameat);
INTERPOSE(intercepted_fstatat, fstatat);
INTERPOSE(intercepted_fstat, fstat);
