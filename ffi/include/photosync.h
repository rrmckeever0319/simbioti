#ifndef PHOTOSYNC_H
#define PHOTOSYNC_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque. Create it on a background queue and do not share it across threads. */
typedef struct PhotosyncConfig PhotosyncConfig;

/* role: 0 = client, 1 = hub.
   peer may be NULL for a hub. listen may be NULL for a client.
   psk_hex is 64 hex characters (32 bytes). Returns NULL on bad input. */
PhotosyncConfig *photosync_config_new(
    const char *folder,
    const char *peer,
    const char *psk_hex,
    int role,
    const char *listen);

void photosync_config_free(PhotosyncConfig *cfg);

/* Recount and rehash the folder, write the local index, return the media count.
   On failure returns -1 and writes a NUL-terminated message into err_buf. */
int photosync_scan(PhotosyncConfig *cfg, char *err_buf, size_t err_buf_len);

/* Blocking sync of missing jpg, jpeg, png, heic, mp4, and mov files.
   A client dials peer. A hub waits for one connection, then returns.
   Returns 0 on success, -1 on failure. */
int photosync_sync_once(PhotosyncConfig *cfg, char *err_buf, size_t err_buf_len);

#ifdef __cplusplus
}
#endif

#endif
