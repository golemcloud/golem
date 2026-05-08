#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include "moonbit.h"

#ifdef _WIN32
#include <windows.h>
#include <direct.h>
#else
#include <dirent.h>
#include <unistd.h>
#endif

MOONBIT_FFI_EXPORT moonbit_bytes_t get_env_var(moonbit_bytes_t key) {
#ifdef _WIN32
    DWORD buf_size = GetEnvironmentVariable((LPSTR)key, NULL, 0);
    if (buf_size == 0) {
        return moonbit_make_bytes(0, 0); // Return empty bytes to indicate None
    }
    moonbit_bytes_t result = moonbit_make_bytes(buf_size - 1, 0);
    GetEnvironmentVariable((LPSTR)key, (LPSTR)result, buf_size);
    return result;
#else
    char* value = getenv((const char*)key);
    if (value == NULL) {
        return moonbit_make_bytes(0, 0); // Return empty bytes to indicate None
    }
    size_t len = strlen(value);
    moonbit_bytes_t result = moonbit_make_bytes(len, 0);
    memcpy(result, value, len);
    return result;
#endif
}

MOONBIT_FFI_EXPORT int32_t get_env_var_exists(moonbit_bytes_t key) {
#ifdef _WIN32
    DWORD buf_size = GetEnvironmentVariable((LPSTR)key, NULL, 0);
    return buf_size != 0; // Variable exists
#else
    char* value = getenv((const char*)key);
    return value != NULL; // Variable exists
#endif
}

MOONBIT_FFI_EXPORT moonbit_bytes_t* get_env_vars() {
#ifdef _WIN32
    // Get environment block
    LPCH env_block = GetEnvironmentStrings();
    if (env_block == NULL) {
        return (moonbit_bytes_t *)moonbit_make_ref_array(0, NULL);
    }

    // Count variables and create array
    int count = 0;
    LPCH env = env_block;
    while (*env) {
        count++;
        env += strlen(env) + 1;
    }

    moonbit_bytes_t* result = (moonbit_bytes_t*)moonbit_make_ref_array(count * 2, NULL);
    
    // Parse variables
    env = env_block;
    int i = 0;
    while (*env) {
        char *equals = strchr(env, '=');
        if (equals != NULL) {
            size_t key_len = equals - env;
            size_t val_len = strlen(equals + 1);
            
            moonbit_bytes_t key = moonbit_make_bytes(key_len, 0);
            memcpy(key, env, key_len);
            
            moonbit_bytes_t value = moonbit_make_bytes(val_len, 0);
            memcpy(value, equals + 1, val_len);

            result[i * 2] = key;
            result[i * 2 + 1] = value;
        }
        env += strlen(env) + 1;
        i++;
    }

    FreeEnvironmentStrings(env_block);
    return result;
#else
    // environ is a pointer to an array of environment variables
    extern char **environ;
    // Count the number of environment variables
    int count = 0;
    char **env = environ;
    while (*env != NULL) {
        count++;
        env++;
    }

    // Create an array to store environment variable key-value pairs
    // Array size is twice the number of variables since we need to store both key and value
    moonbit_bytes_t* result = (moonbit_bytes_t*)moonbit_make_ref_array(count * 2, NULL);
    env = environ;
    int i = 0;
    while (*env != NULL) {
        // Find the '=' character in the environment variable string
        char *equals = strchr(*env, '=');
        if (equals != NULL) {
            // Calculate lengths of key and value
            size_t key_len = equals - *env;
            size_t val_len = strlen(equals + 1);
            
            // Create bytes object for key and copy data
            moonbit_bytes_t key = moonbit_make_bytes(key_len, 0);
            memcpy(key, *env, key_len);
            
            // Create bytes object for value and copy data
            moonbit_bytes_t value = moonbit_make_bytes(val_len, 0);
            memcpy(value, equals + 1, val_len);

            // Store key and value in result array
            // Even indices store keys, odd indices store values
            result[i * 2] = key;
            result[i * 2 + 1] = value;
        }
        env++;
        i++;
    }
    return result;
#endif
}

MOONBIT_FFI_EXPORT void set_env_var(moonbit_bytes_t key, moonbit_bytes_t value) {
#ifdef _WIN32
    SetEnvironmentVariable(key, value);
#else
    setenv((const char*)key, (const char*)value, 1);
#endif
}

MOONBIT_FFI_EXPORT void unset_env_var(moonbit_bytes_t key) {
#ifdef _WIN32
    SetEnvironmentVariable(key, NULL);
#else
    unsetenv((const char*)key);
#endif
}
