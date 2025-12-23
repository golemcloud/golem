#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include "c_api1.h"

int32_t main(void) {
    return 0;
}

#define DATA_SIZE 536870912
static char DATA[DATA_SIZE] = {};

uint64_t exports_c_api1_run(void) {
    printf("DATA:  %lu\n", sizeof(DATA));
    printf("first: %d\n", DATA[0]);
    printf("last:  %d\n", DATA[DATA_SIZE-1]);

    sleep(2);

    return sizeof(DATA);
}
