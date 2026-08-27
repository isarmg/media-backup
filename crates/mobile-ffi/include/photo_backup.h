#ifndef PHOTO_BACKUP_H
#define PHOTO_BACKUP_H

#include <stdbool.h>
#include <stdint.h>

uint64_t pb_open(const char *database_path, const char *config_json);
void pb_close(uint64_t handle);
bool pb_needs(uint64_t handle, const char *source_asset_id, const char *source_resource_id, int64_t modified_ms);
char *pb_enqueue(uint64_t handle, const char *input_json);
char *pb_next(uint64_t handle, const char *staging_root);
char *pb_mark_upload(uint64_t handle, const char *job_id, const char *upload_id);
char *pb_mark_part(uint64_t handle, const char *job_id, uint32_t part_index);
char *pb_mark_complete(uint64_t handle, const char *job_id);
char *pb_mark_failed(uint64_t handle, const char *job_id, const char *error, bool retryable);
char *pb_stats(uint64_t handle);
const char *pb_last_error(void);
void pb_string_free(char *value);

#endif
