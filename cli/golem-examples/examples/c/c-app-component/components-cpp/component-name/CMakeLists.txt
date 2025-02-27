if (NOT DEFINED ENV{WASI_SDK_PATH})
    message(FATAL_ERROR "WASI_SDK_PATH environment variable is not set.")
endif ()
set(CMAKE_TOOLCHAIN_FILE "$ENV{WASI_SDK_PATH}/share/cmake/wasi-sdk.cmake")
cmake_minimum_required(VERSION 3.26.0)

project(component-name)

set(CMAKE_CXX_STANDARD 20)

add_subdirectory(../../common-cpp/lib common_lib)

add_executable(component_name.wasm
        src/component.cpp
        binding/component_name.c
        binding/component_name_component_type.o
)

target_include_directories(component_name.wasm
        PRIVATE include
        PRIVATE binding
)

target_link_libraries(component_name.wasm
        PRIVATE common_lib
)
