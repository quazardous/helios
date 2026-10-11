/*
 * darktable's own params migrations, as an oracle for Helios's: reads lines
 * "MODULE VERSION HEX" on stdin and prints, for each, the params darktable's
 * legacy_params gives at the module's current version ("MODULE VERSION
 * CURRENT HEX"), or "MODULE VERSION FAIL" when darktable has no way.
 *
 *   tools/dt_legacy_oracle.sh DARKTABLE_SRC DARKTABLE_BUILD < pairs > out
 *
 * Only the plugin's legacy_params runs, with no module instance: it is
 * meant for modules whose migrations do not use one.
 */
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "common/introspection.h"

typedef int (*legacy_fn)(void *self, const void *old, int old_version, void **new_params,
                         int *new_size, int *new_version);
typedef int (*init_fn)(void *self, int api_version);
typedef dt_introspection_t *(*get_fn)(void);

int main(int argc, char **argv)
{
  if(argc != 2)
  {
    fprintf(stderr, "usage: %s PLUGIN_DIR < pairs\n", argv[0]);
    return 2;
  }
  char module[128], hex[1 << 16];
  int version;
  while(scanf("%127s %d %65535s", module, &version, hex) == 3)
  {
    char path[4096];
    snprintf(path, sizeof(path), "%s/lib%s.so", argv[1], module);
    void *lib = dlopen(path, RTLD_LAZY | RTLD_LOCAL);
    legacy_fn legacy = lib ? (legacy_fn)dlsym(lib, "legacy_params") : NULL;
    init_fn init = lib ? (init_fn)dlsym(lib, "introspection_init") : NULL;
    get_fn get = lib ? (get_fn)dlsym(lib, "get_introspection") : NULL;
    if(!legacy || !init || !get || init(NULL, DT_INTROSPECTION_VERSION))
    {
      printf("%s %d FAIL\n", module, version);
      continue;
    }
    const int current = get()->params_version;
    const size_t len = strlen(hex) / 2;
    unsigned char *params = malloc(len ? len : 1);
    for(size_t i = 0; i < len; i++) sscanf(hex + 2 * i, "%2hhx", &params[i]);
    int v = version, size = (int)len, ok = 1;
    while(v < current)
    {
      void *next = NULL;
      int next_size = 0, next_version = 0;
      if(legacy(NULL, params, v, &next, &next_size, &next_version) || next_version <= v)
      {
        ok = 0;
        break;
      }
      free(params);
      params = next;
      size = next_size;
      v = next_version;
    }
    if(!ok || v != current)
      printf("%s %d FAIL\n", module, version);
    else
    {
      printf("%s %d %d ", module, version, current);
      for(int i = 0; i < size; i++) printf("%02x", params[i]);
      printf("\n");
    }
    free(params);
  }
  return 0;
}
