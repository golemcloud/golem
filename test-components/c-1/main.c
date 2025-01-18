#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "c_api1.h"

int32_t main(void) {
    return 0;
}

int32_t exports_c_api1_run(void) {
    printf("Hello World!\n");
    return 100;
}

void exports_c_api1_print(c_api1_string_t *s) {
    char* buf = malloc(s->len + 1);
    memset(buf, 0, s->len + 1);
    strncpy(buf, s->ptr, s->len);

    time_t t = time(NULL);
    struct tm tm = *localtime(&t);

    printf("%s %d\n", buf, tm.tm_year + 1900);
}
