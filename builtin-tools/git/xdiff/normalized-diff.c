/* SPDX-License-Identifier: Apache-2.0 */
#include "git-compat-util.h"
#include "xdiff.h"

/* One bounded arena per calculation; no libc, WASI or platform imports. */
static _Alignas(8) unsigned char arena[32 * 1024 * 1024];
static size_t used;
void reset(void) { used = 0; }
void *xmalloc(size_t size) {
  if (size > sizeof(arena) - 16 || used > sizeof(arena) - size - 16)
    __builtin_trap();
  size_t *header = (size_t *)(arena + used);
  *header = size;
  used += (size + 15) & ~(size_t)7;
  return header + 2;
}
void *allocate(size_t size) { return xmalloc(size); }
void free(void *pointer) { /* The arena is reset between calculations. */ }
void *memcpy(void *target, const void *source, size_t size) {
  unsigned char *out = target;
  const unsigned char *in = source;
  for (size_t i = 0; i < size; i++) out[i] = in[i];
  return target;
}
void *memmove(void *target, const void *source, size_t size) {
  unsigned char *out = target;
  const unsigned char *in = source;
  if (out <= in) return memcpy(target, source, size);
  for (size_t i = size; i > 0; i--) out[i - 1] = in[i - 1];
  return target;
}
void *memset(void *target, int value, size_t size) {
  unsigned char *out = target;
  for (size_t i = 0; i < size; i++) out[i] = value;
  return target;
}
int memcmp(const void *left, const void *right, size_t size) {
  const unsigned char *a = left, *b = right;
  for (size_t i = 0; i < size; i++)
    if (a[i] != b[i]) return (int)a[i] - b[i];
  return 0;
}
void *memchr(const void *buffer, int value, size_t size) {
  const unsigned char *bytes = buffer;
  for (size_t i = 0; i < size; i++)
    if (bytes[i] == (unsigned char)value) return (void *)(bytes + i);
  return NULL;
}
size_t strlen(const char *string) {
  size_t size = 0;
  while (string[size]) size++;
  return size;
}
int strncmp(const char *left, const char *right, size_t size) {
  for (size_t i = 0; i < size; i++) {
    int difference = (unsigned char)left[i] - (unsigned char)right[i];
    if (difference || !left[i]) return difference;
  }
  return 0;
}
void *xcalloc(size_t count, size_t size) {
  if (size && count > SIZE_MAX / size) __builtin_trap();
  return memset(xmalloc(count * size), 0, count * size);
}
void *xrealloc(void *pointer, size_t size) {
  void *result = xmalloc(size);
  if (pointer) {
    size_t previous = ((size_t *)pointer)[-2];
    memcpy(result, pointer, previous < size ? previous : size);
  }
  return result;
}
typedef struct { unsigned char *bytes; size_t size, capacity; } output_t;
static int emit(void *context, mmbuffer_t *buffers, int count) {
  output_t *output = context;
  for (int i = 0; i < count; i++) {
    /* xdl_emit_diffrec's third buffer is the missing-newline marker. */
    if (i == 2) continue;
    for (long j = 0; j < buffers[i].size; j++) {
      unsigned char byte = buffers[i].ptr[j];
      if (isspace(byte)) continue;
      if (output->size == output->capacity) return -1;
      output->bytes[output->size++] = byte;
    }
  }
  return 0;
}
int normalized_diff(char *old, int old_size, char *new, int new_size,
                    unsigned char *out, int capacity) {
  mmfile_t before = { old, old_size }, after = { new, new_size };
  xpparam_t parameters = { 0 };
  xdemitconf_t configuration = { 0 };
  configuration.ctxlen = 3;
  configuration.flags = XDL_EMIT_NO_HUNK_HDR;
  output_t output = { out, 0, capacity };
  xdemitcb_t callback = { .priv = &output, .out_line = emit };
  if (xdl_diff(&before, &after, &parameters, &configuration, &callback) < 0)
    return -1;
  return output.size;
}
