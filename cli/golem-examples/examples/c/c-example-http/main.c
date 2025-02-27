#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
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


void set_string(component_name_string_t* ret, const char* str) {
    ret->ptr = (uint8_t*)str;
    ret->len = strlen(str);
}

void set_string_field(wasi_http_types_field_value_t* ret, const char* str) {
    ret->ptr = (uint8_t*)str;
    ret->len = strlen(str);
}

#define log(...) { fprintf (stderr, __VA_ARGS__); fflush(stderr); }

void exports_pack_name_api_send(component_name_string_t *ret) {
    log("Setting up the outgoing request\n");
    // Setting up the outgoing request
    wasi_http_types_own_fields_t headers;
    wasi_http_types_header_error_t headers_err;
    component_name_list_tuple2_field_key_field_value_t entries;
    entries.ptr = malloc(2 * sizeof(component_name_tuple2_field_key_field_value_t));
    entries.len = 2;
    set_string(&entries.ptr[0].f0, "Content-Type");
    set_string_field(&entries.ptr[0].f1, "application/json");
    set_string(&entries.ptr[1].f0, "Accept");
    set_string_field(&entries.ptr[1].f1, "application/json");

    if (!wasi_http_types_static_fields_from_list(&entries, &headers, &headers_err)) {
        set_string(ret, "Failed to create header list");
        return;
    }

    log("Created the header list\n");

    wasi_http_types_own_outgoing_request_t request = wasi_http_types_constructor_outgoing_request(headers);

    log("Created the request\n");

    wasi_http_types_method_t method;
    method.tag = WASI_HTTP_TYPES_METHOD_POST;
    if (!wasi_http_types_method_outgoing_request_set_method(
        wasi_http_types_borrow_outgoing_request(request),
        &method
    )) {
        set_string(ret, "Failed to set method");
        wasi_http_types_outgoing_request_drop_own(request);
        return;
    }

    component_name_string_t path;
    set_string(&path, "/post");
    if (!wasi_http_types_method_outgoing_request_set_path_with_query(
        wasi_http_types_borrow_outgoing_request(request),
        &path
    )) {
        set_string(ret, "Failed to set path");
        wasi_http_types_outgoing_request_drop_own(request);
        return;
    }

    wasi_http_types_scheme_t scheme;
    scheme.tag = WASI_HTTP_TYPES_SCHEME_HTTPS;
    if (!wasi_http_types_method_outgoing_request_set_scheme(
        wasi_http_types_borrow_outgoing_request(request),
        &scheme
    )) {
        set_string(ret, "Failed to set scheme");
        wasi_http_types_outgoing_request_drop_own(request);
        return;
    }

    component_name_string_t authority;
    set_string(&authority, "httpbin.org");
    if (!wasi_http_types_method_outgoing_request_set_authority(
        wasi_http_types_borrow_outgoing_request(request),
        &authority
    )) {
        set_string(ret, "Failed to set authority");
        wasi_http_types_outgoing_request_drop_own(request);
        return;
    }

    wasi_http_types_own_outgoing_body_t out_body;
    if (!wasi_http_types_method_outgoing_request_body(wasi_http_types_borrow_outgoing_request(request), &out_body)) {
        set_string(ret, "Failed to get outgoing body");
        wasi_http_types_outgoing_request_drop_own(request);
        return;
    }

    wasi_http_types_own_output_stream_t out_body_stream;
    if (!wasi_http_types_method_outgoing_body_write(wasi_http_types_borrow_outgoing_body(out_body), &out_body_stream)) {
        set_string(ret, "Failed to get outgoing body stream");
        wasi_http_types_outgoing_body_drop_own(out_body);
        wasi_http_types_outgoing_request_drop_own(request);
        return;
    }

    log("Writing the outgoing request stream\n");

    wasi_io_streams_stream_error_t stream_err;
    component_name_list_u8_t body_data;
    body_data.ptr = malloc(256);
    sprintf((char*)body_data.ptr, "{ \"count\": %llu }", total);
    body_data.len = strlen((char*)body_data.ptr);
    if (!wasi_io_streams_method_output_stream_blocking_write_and_flush(
        wasi_io_streams_borrow_output_stream(out_body_stream), &body_data, &stream_err)) {
        set_string(ret, "Failed to write body");
        wasi_io_streams_output_stream_drop_own(out_body_stream);
        wasi_http_types_outgoing_body_drop_own(out_body);
        wasi_http_types_outgoing_request_drop_own(request);
        return;
    }

    log("Finished writing the outgoing request stream\n");

    wasi_io_streams_output_stream_drop_own(out_body_stream);

    wasi_http_types_error_code_t err;
    if (!wasi_http_types_static_outgoing_body_finish(out_body, NULL, &err)) {
        set_string(ret, "Failed to finish body");
        wasi_http_types_outgoing_request_drop_own(request);
        return;
    }

    log("Finished setting up the request\n");

    // Sending the request

    wasi_http_types_own_request_options_t request_options = wasi_http_types_constructor_request_options();
    wasi_http_types_duration_t timeout = 5000000000; // 5 seconds (in ns)

    log("Setting the request options\n");

    if (!wasi_http_types_method_request_options_set_connect_timeout(
        wasi_http_types_borrow_request_options(request_options),
        &timeout
    )) {
        log("Failed to set connect timeout\n")
        set_string(ret, "Failed to set connect timeout");
        wasi_http_types_request_options_drop_own(request_options);
        wasi_http_types_outgoing_request_drop_own(request);
        return;
    }

    if (!wasi_http_types_method_request_options_set_first_byte_timeout(
        wasi_http_types_borrow_request_options(request_options),
        &timeout
    )) {
        log("Failed to set first byte timeout\n")
        set_string(ret, "Failed to set first byte timeout");
        wasi_http_types_request_options_drop_own(request_options);
        wasi_http_types_outgoing_request_drop_own(request);
        return;
    }

    if (!wasi_http_types_method_request_options_set_between_bytes_timeout(
        wasi_http_types_borrow_request_options(request_options),
        &timeout
    )) {
        log("Failed to set between-bytes timeout\n")
        set_string(ret, "Failed to set between-bytes timeout");
        wasi_http_types_request_options_drop_own(request_options);
        wasi_http_types_outgoing_request_drop_own(request);
        return;
    }

    log("Sending the request\n");

    wasi_http_outgoing_handler_own_future_incoming_response_t future_response;
    wasi_http_outgoing_handler_error_code_t err_code;
    if (!wasi_http_outgoing_handler_handle(request, &request_options, &future_response, &err_code)) {
        set_string(ret, "Failed to send request");
        return;
    }

    // Awaiting for the response

    bool got_response = false;
    wasi_http_types_own_incoming_response_t response;

    while (!got_response) {
        wasi_http_types_result_result_own_incoming_response_error_code_void_t res;
        if (wasi_http_types_method_future_incoming_response_get(wasi_http_types_borrow_future_incoming_response(future_response), &res)) {
            if (!res.is_err && !res.val.ok.is_err) {
                log("Got response\n");
                response = res.val.ok.val.ok;
                got_response = true;
            } else if (!res.is_err && res.val.ok.is_err) {
                log("Returned with an error code: %u\n", res.val.ok.val.err.tag);
                set_string(ret, "Returned with error code");
                wasi_http_types_future_incoming_response_drop_own(future_response);
                return;
            } else {
                log("Returned with an error\n");
                set_string(ret, "Returned with error");
                wasi_http_types_future_incoming_response_drop_own(future_response);
                return;
            }
        } else {
            log("No result yet, polling\n");

            wasi_http_types_own_pollable_t pollable = wasi_http_types_method_future_incoming_response_subscribe(
                wasi_http_types_borrow_future_incoming_response(future_response)
            );
            wasi_io_poll_list_borrow_pollable_t pollable_list;
            pollable_list.len = 1;
            pollable_list.ptr = malloc(sizeof(wasi_io_poll_borrow_pollable_t));
            pollable_list.ptr[0] = wasi_io_poll_borrow_pollable(pollable);

            component_name_list_u32_t poll_result;
            wasi_io_poll_poll(&pollable_list, &poll_result);
            wasi_io_poll_pollable_drop_own(pollable);
        }
    }

    // Processing the response

    wasi_http_types_status_code_t status = wasi_http_types_method_incoming_response_status(
        wasi_http_types_borrow_incoming_response(response)
    );

    log("Got response with status %u\n", status);

    wasi_http_types_own_incoming_body_t incoming_body;
    if (!wasi_http_types_method_incoming_response_consume(wasi_http_types_borrow_incoming_response(response), &incoming_body)) {
        set_string(ret, "Failed to consume response");
        wasi_http_types_incoming_response_drop_own(response);
        return;
    }

    wasi_http_types_own_input_stream_t incoming_body_stream;
    if (!wasi_http_types_method_incoming_body_stream(wasi_http_types_borrow_incoming_body(incoming_body), &incoming_body_stream)) {
        set_string(ret, "Failed to get body stream");
        wasi_http_types_incoming_body_drop_own(incoming_body);
        wasi_http_types_incoming_response_drop_own(response);
        return;
    }

    bool eof = false;
    uint8_t *full_body = malloc(0);
    uint64_t len = 0;

    while (!eof) {
        component_name_list_u8_t chunk;
        wasi_io_streams_stream_error_t stream_err;
        if (wasi_io_streams_method_input_stream_blocking_read(wasi_io_streams_borrow_input_stream(incoming_body_stream), 1024, &chunk, &stream_err)) {
            len += chunk.len;
            full_body = realloc(full_body, len);
            memcpy(full_body + len - chunk.len, chunk.ptr, chunk.len);
        } else {
            if (stream_err.tag == WASI_IO_STREAMS_STREAM_ERROR_CLOSED) {
                eof = true;
            } else {
                set_string(ret, "Failed to read from body stream");
                wasi_io_streams_input_stream_drop_own(incoming_body_stream);
                wasi_http_types_incoming_body_drop_own(incoming_body);
                wasi_http_types_incoming_response_drop_own(response);
                return;
            }
        }
    }

    wasi_io_streams_input_stream_drop_own(incoming_body_stream);
    wasi_http_types_incoming_body_drop_own(incoming_body);
    wasi_http_types_incoming_response_drop_own(response);

    log("Returning %llu characters\n", len)

    ret->ptr = full_body;
    ret->len = len;
}
