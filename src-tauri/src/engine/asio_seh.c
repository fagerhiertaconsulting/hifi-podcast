/* Windows SEH around ASIO bufferSwitch. Rust catch_unwind does not catch
   ACCESS_VIOLATION (0xC0000005). Keep the process alive so native can fail
   to DoP instead of killing the UI. */
#include <stdint.h>
#include <windows.h>

typedef void (*hifi_asio_fill_cb)(int32_t);

int32_t hifi_asio_seh_invoke(hifi_asio_fill_cb cb, int32_t index) {
    if (!cb) {
        return (int32_t)0xC0000005;
    }
    __try {
        cb(index);
        return 0;
    } __except (EXCEPTION_EXECUTE_HANDLER) {
        return (int32_t)GetExceptionCode();
    }
}
