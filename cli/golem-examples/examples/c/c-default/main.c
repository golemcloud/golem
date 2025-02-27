#include <stdio.h>
#include <time.h>

#include "component_name/component_name.h"

int32_t main(void) {
    return 0;
}

// Component state
static uint64_t total = 0;

// Implementation of the exported functions.
// See component_name.h for the generated function signatures.
void exports_pack_name_api_add(uint64_t value) {
    total += value;
}

uint64_t exports_pack_name_api_get() {
    return total;
}
