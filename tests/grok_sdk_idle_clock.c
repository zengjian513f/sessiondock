/* Browser-fixture process clock only. Production has no idle-time override.
 * Advance CLOCK_MONOTONIC in the isolated server through a private marker;
 * wall-clock timestamps and the surrounding browser/test process are unchanged.
 */
#define _GNU_SOURCE
#include <dlfcn.h>
#include <fcntl.h>
#include <stdlib.h>
#include <sys/syscall.h>
#include <time.h>
#include <unistd.h>

int clock_gettime(clockid_t clock_id, struct timespec *value) {
    static int (*native_clock)(clockid_t, struct timespec *);
    if (!native_clock) native_clock = dlsym(RTLD_NEXT, "clock_gettime");
    int result = native_clock(clock_id, value);
    if (result || clock_id != CLOCK_MONOTONIC) return result;
    const char *marker = getenv("SESSIONDOCK_TEST_CLOCK_FILE");
    if (!marker) return result;
    int fd = syscall(SYS_openat, AT_FDCWD, marker, O_RDONLY | O_CLOEXEC, 0);
    if (fd < 0) return result;
    char buffer[40];
    long length = syscall(SYS_read, fd, buffer, sizeof(buffer) - 1);
    syscall(SYS_close, fd);
    if (length > 0) {
        buffer[length] = 0;
        long seconds = strtol(buffer, 0, 10);
        if (seconds > 0) value->tv_sec += seconds;
    }
    return result;
}
