#include <iostream>
// Include this for using the common_lib
// #include "common_lib.h"
#include "component_name.h"

static uint64_t total = 0;

void exports_pack_name_exports_component_name_api_add(uint64_t value) {
    // Example common lib call
    // std::cout << common_lib::example_common_function();
    total += value;
}

uint64_t exports_component_name_exports_component_name_api_get() {
    return total;
}

int32_t main(void) {
    return 0;
}
