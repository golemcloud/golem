#include <moonbit.h>

#ifdef _WIN32

#include <windows.h>

MOONBIT_FFI_EXPORT int64_t instant_now_ffi() {
  LARGE_INTEGER t;
  QueryPerformanceCounter(&t);
  return t.QuadPart;
}

MOONBIT_FFI_EXPORT double instant_as_secs_f64_ffi(int64_t t) {
  LARGE_INTEGER freq;
  QueryPerformanceFrequency(&freq);
  return ((double)t) / ((double)freq.QuadPart);
}

#else

#include <time.h>

void timespec_delete(void *_t) {}

MOONBIT_FFI_EXPORT struct timespec *instant_now_ffi() {
  struct timespec *res =
      moonbit_make_external_object(&timespec_delete, sizeof(struct timespec));
  clock_gettime(CLOCK_MONOTONIC, res);
  return res;
}

MOONBIT_FFI_EXPORT double instant_as_secs_f64_ffi(struct timespec *t) {
  return ((double)t->tv_sec) + ((double)t->tv_nsec) * 1e-9;
}

#endif
