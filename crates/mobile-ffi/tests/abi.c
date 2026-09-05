#include "media_backup_ffi_v2.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>

static void release(SarmgFfiResultV2 *out) {
    assert(out->abi_revision == SARMG_FFI_ABI_REVISION);
    assert(sarmg_ffi_result_free_v2(out) == SARMG_FFI_OK);
    assert(out->bytes.data == NULL && out->bytes.length == 0 && out->value == 0);
    assert(sarmg_ffi_result_free_v2(out) == SARMG_FFI_OK);
}

int main(int argc, char **argv) {
    assert(argc == 2);
    assert(mb_ffi_abi_revision() == SARMG_FFI_ABI_REVISION);
    SarmgFfiResultV2 out = {0};
    assert(mb_stats_v2(0, NULL) == SARMG_FFI_INVALID_ARGUMENT);
    assert(mb_stats_v2(0, &out) == SARMG_FFI_INVALID_HANDLE);
    assert(out.status == SARMG_FFI_INVALID_HANDLE && out.value == 0);
    assert(out.bytes.length == strlen("invalid handle"));
    assert(memcmp(out.bytes.data, "invalid handle", out.bytes.length) == 0);
    release(&out);
    const uint8_t raw[] = {'a'}; /* Deliberately not NUL terminated. */
    assert(mb_needs_v2(0, raw, sizeof(raw), raw, sizeof(raw), 0, &out) == SARMG_FFI_INVALID_HANDLE);
    release(&out);
    assert(mb_enqueue_v2(0, NULL, 1, &out) == SARMG_FFI_INVALID_ARGUMENT);
    release(&out);
    assert(mb_next_v2(0, raw, sizeof(raw), &out) == SARMG_FFI_INVALID_ARGUMENT);
    release(&out);
    assert(mb_mark_upload_v2(0, raw, 1, raw, 1, &out) == SARMG_FFI_INVALID_HANDLE);
    release(&out);
    assert(mb_mark_part_v2(0, raw, 1, UINT32_MAX, &out) == SARMG_FFI_INVALID_HANDLE);
    release(&out);
    assert(mb_mark_complete_v2(0, raw, 1, &out) == SARMG_FFI_INVALID_HANDLE);
    release(&out);
    assert(mb_mark_failed_v2(0, raw, 1, raw, 1, 2, &out) == SARMG_FFI_INVALID_ARGUMENT);
    release(&out);
    assert(mb_mark_failed_v2(0, raw, 1, raw, 1, 1, &out) == SARMG_FFI_INVALID_HANDLE);
    release(&out);
    assert(mb_open_v2(NULL, 1, NULL, 0, &out) == SARMG_FFI_INVALID_ARGUMENT);
    release(&out);
    assert(mb_open_v2(raw, SIZE_MAX, raw, 1, &out) == SARMG_FFI_INVALID_ARGUMENT);
    release(&out);
    char path[4096];
    int length = snprintf(path, sizeof(path), "%s/agent-v0.2-r1.sqlite", argv[1]);
    assert(length > 0 && (size_t)length < sizeof(path));
    const char config[] = "{\"product\":\"media-backup\",\"application_version\":\"0.2.0\",\"revision\":1,\"state_epoch\":\"media-backup-mobile-v0.2-r1\",\"part_size\":16777216}";
    assert(mb_open_v2((const uint8_t *)path, (size_t)length, (const uint8_t *)config, sizeof(config) - 1, &out) == SARMG_FFI_OK);
    uint64_t first = out.value;
    assert(first != 0);
    release(&out);
    assert(mb_needs_v2(first, raw, 1, raw, 1, 1, &out) == SARMG_FFI_OK && out.value == 1);
    release(&out);
    assert(mb_stats_v2(first, &out) == SARMG_FFI_OK && out.bytes.length > 0);
    assert(out.bytes.data[0] == '{');
    release(&out);
    assert(mb_close_v2(first, &out) == SARMG_FFI_OK);
    release(&out);
    assert(mb_close_v2(first, &out) == SARMG_FFI_INVALID_HANDLE);
    release(&out);
    assert(mb_open_v2((const uint8_t *)path, (size_t)length, (const uint8_t *)config, sizeof(config) - 1, &out) == SARMG_FFI_OK);
    uint64_t second = out.value;
    assert(second != first);
    release(&out);
    assert(mb_stats_v2(first, &out) == SARMG_FFI_INVALID_HANDLE);
    release(&out);
    assert(mb_close_v2(second, &out) == SARMG_FFI_OK);
    release(&out);
    puts("current C ABI: lengths, statuses, owned results, all exports and stale handles passed");
    return 0;
}
