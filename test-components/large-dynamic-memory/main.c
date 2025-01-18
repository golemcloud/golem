#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include "c_api1.h"

int32_t main(void) {
    return 0;
}

#define PAGE_SIZE 1024*1024
#define COUNT 512

uint64_t exports_c_api1_run(void) {
    for (int i = 0; i < COUNT; i++) {
        char* DATA = malloc(PAGE_SIZE);
        printf("page %d first: %d\n", i, DATA[0]);
        printf("page %d last:  %d\n", i, DATA[PAGE_SIZE-1]);

        usleep(5);
    }

    return 0;
}
