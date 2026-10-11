/* SPDX-License-Identifier: Apache-2.0 */
#ifndef GOLEM_XDIFF_COMPAT_H
#define GOLEM_XDIFF_COMPAT_H
#include <stddef.h>
#include <stdint.h>
#include <stdbool.h>
#include <limits.h>
#include <string.h>

void *xmalloc(size_t size);
void *xcalloc(size_t count, size_t size);
void *xrealloc(void *pointer, size_t size);
void free(void *pointer);
#define UNUSED __attribute__((unused))
#define BUG(...) __builtin_trap()
#define isspace(c) ((c) == 9 || (c) == 10 || (c) == 13 || (c) == 32)
#define isalpha(c) (((c) >= 'A' && (c) <= 'Z') || ((c) >= 'a' && (c) <= 'z'))
static inline bool signed_add_overflows(long a, long b) {
  return (b > 0 && a > LONG_MAX - b) || (b < 0 && a < LONG_MIN - b);
}
typedef struct { int unused; } regex_t;
typedef struct { long rm_so, rm_eo; } regmatch_t;
static inline int regexec_buf(const regex_t *regex, const char *buffer,
                             size_t length, size_t count,
                             regmatch_t *matches, int flags) {
  __builtin_trap();
}
#endif
