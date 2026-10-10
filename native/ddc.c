/* Small ABI boundary: compile against the distribution's libddcutil headers.
 * Only the isolated hardware process calls these functions, serially. */
#include <ddcutil_c_api.h>
#include <stdint.h>
#include <errno.h>
#include <stdlib.h>
#include <string.h>

#define MC_MAX_DISPLAYS 64
typedef struct {
    char model[64];
    char serial[64];
    char connector[64];
    uint8_t edid[128];
    int bus;
} MC_Info;
static DDCA_Display_Ref references[MC_MAX_DISPLAYS];
static int reference_count;
static int discovered;

int mc_init(void) {
    /* Explicitly disable the library's polling and user config overrides. */
    return ddca_init2("--disable-watch-displays", DDCA_SYSLOG_ERROR,
                     DDCA_INIT_OPTIONS_DISABLE_CONFIG_FILE, NULL);
}

const char *mc_error(int code) { return ddca_rc_desc(code); }

int mc_discover(MC_Info *out, int capacity) {
    reference_count = 0;
    if (discovered) {
        int rc = ddca_redetect_displays();
        if (rc) return rc;
    }
    discovered = 1;
    DDCA_Display_Ref *refs = NULL;
    int rc = ddca_get_display_refs(true, &refs);
    if (rc) return rc;
    if (!refs) return 0;
    for (int i = 0; refs[i] && reference_count < capacity && reference_count < MC_MAX_DISPLAYS; i++) {
        DDCA_Display_Info2 *info = NULL;
        rc = ddca_get_display_info2(refs[i], &info);
        if (rc || !info) continue;
        MC_Info *dest = &out[reference_count];
        memset(dest, 0, sizeof(*dest));
        strncpy(dest->model, info->model_name, sizeof(dest->model) - 1);
        strncpy(dest->serial, info->sn, sizeof(dest->serial) - 1);
        strncpy(dest->connector, info->drm_card_connector, sizeof(dest->connector) - 1);
        memcpy(dest->edid, info->edid_bytes, sizeof(dest->edid));
        dest->bus = info->path.path.i2c_busno;
        references[reference_count++] = refs[i];
        ddca_free_display_info2(info);
    }
    free(refs);
    return reference_count;
}

int mc_read(int index, uint16_t *current, uint16_t *maximum) {
    if (index < 0 || index >= reference_count) return -EINVAL;
    DDCA_Display_Handle handle = NULL;
    int rc = ddca_open_display2(references[index], false, &handle);
    if (rc) return rc;
    DDCA_Non_Table_Vcp_Value value;
    rc = ddca_get_non_table_vcp_value(handle, 0x10, &value);
    if (!rc) {
        *current = (value.sh << 8) | value.sl;
        *maximum = (value.mh << 8) | value.ml;
    }
    ddca_close_display(handle);
    return rc;
}

int mc_write(int index, uint16_t value) {
    if (index < 0 || index >= reference_count) return -EINVAL;
    DDCA_Display_Handle handle = NULL;
    int rc = ddca_open_display2(references[index], false, &handle);
    if (rc) return rc;
    rc = ddca_set_non_table_vcp_value(handle, 0x10, value >> 8, value & 0xff);
    ddca_close_display(handle);
    return rc;
}
