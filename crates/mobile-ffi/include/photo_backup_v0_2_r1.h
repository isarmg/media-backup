#ifndef PHOTO_BACKUP_V0_2_R1_H
#define PHOTO_BACKUP_V0_2_R1_H

#include <stdbool.h>
#include <stdint.h>

/* Photo Backup mobile contract epoch: product photo-backup, version 0.2.0, revision 1. */
uint64_t pb_v0_2_r1_open(const char *database_path, const char *config_json);
void pb_v0_2_r1_close(uint64_t handle);
bool pb_v0_2_r1_needs(uint64_t handle, const char *source_asset_id, const char *source_resource_id, int64_t modified_ms);
char *pb_v0_2_r1_enqueue(uint64_t handle, const char *input_json);
char *pb_v0_2_r1_next(uint64_t handle, const char *staging_root);
char *pb_v0_2_r1_mark_upload(uint64_t handle, const char *job_id, const char *upload_id);
char *pb_v0_2_r1_mark_part(uint64_t handle, const char *job_id, uint32_t part_index);
char *pb_v0_2_r1_mark_complete(uint64_t handle, const char *job_id);
char *pb_v0_2_r1_mark_failed(uint64_t handle, const char *job_id, const char *error, bool retryable);
char *pb_v0_2_r1_stats(uint64_t handle);
const char *pb_v0_2_r1_last_error(void);
void pb_v0_2_r1_string_free(char *value);

#endif
