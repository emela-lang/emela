#include <stdint.h>
#include <stdio.h>
#include <time.h>

static uintptr_t emela_ok(uintptr_t value) {
    return (value << 32) | 0u;
}

static uintptr_t emela_err(uint32_t error_tag) {
    return ((uintptr_t)error_tag << 32) | 1u;
}

uintptr_t emela_write_stdout_utf8(const char *value) {
    if (fputs(value, stdout) < 0) {
        return emela_err(4);
    }
    return emela_ok(0);
}

uintptr_t emela_read_stdin_utf8(void) {
    return emela_err(4);
}

int32_t emela_now_i32(void) {
    return (int32_t)time(NULL);
}
