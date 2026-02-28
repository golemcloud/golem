#include <stdint.h>
#include "moonbit.h"

MOONBIT_FFI_EXPORT int32_t moonbitlang_x_path_is_windows() {
#ifdef _WIN32
    return 1;
#else
    return 0;
#endif
}
