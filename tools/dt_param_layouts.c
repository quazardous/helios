/*
 * Prints the parameter layouts of darktable's processing modules as JSON:
 * for each module, its params version and every field with its type,
 * offset, size, range, default and enum values, as darktable's own
 * introspection (generated at build time from the params structs) gives
 * them. Helios decodes XMP history params with this, without darktable.
 *
 *   tools/dt_param_layouts.sh DARKTABLE_BUILD > crates/helios-xmp/layouts.json
 *
 * It loads each module's plugin library and reads its introspection data;
 * no module code runs beyond the introspection initializer.
 */
#include <dirent.h>
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "common/introspection.h"

typedef int (*init_fn)(void *self, int api_version);
typedef dt_introspection_t *(*get_fn)(void);

static void json_string(const char *s)
{
  putchar('"');
  for(; s && *s; s++)
  {
    if(*s == '"' || *s == '\\') printf("\\%c", *s);
    else if((unsigned char)*s < 0x20) printf("\\u%04x", *s);
    else putchar(*s);
  }
  putchar('"');
}

static const char *type_name(dt_introspection_type_t t)
{
  switch(t)
  {
    case DT_INTROSPECTION_TYPE_OPAQUE: return "opaque";
    case DT_INTROSPECTION_TYPE_FLOAT: return "float";
    case DT_INTROSPECTION_TYPE_DOUBLE: return "double";
    case DT_INTROSPECTION_TYPE_FLOATCOMPLEX: return "float_complex";
    case DT_INTROSPECTION_TYPE_CHAR: return "char";
    case DT_INTROSPECTION_TYPE_INT8: return "int8";
    case DT_INTROSPECTION_TYPE_UINT8: return "uint8";
    case DT_INTROSPECTION_TYPE_SHORT: return "short";
    case DT_INTROSPECTION_TYPE_USHORT: return "ushort";
    case DT_INTROSPECTION_TYPE_INT: return "int";
    case DT_INTROSPECTION_TYPE_UINT: return "uint";
    case DT_INTROSPECTION_TYPE_LONG: return "long";
    case DT_INTROSPECTION_TYPE_ULONG: return "ulong";
    case DT_INTROSPECTION_TYPE_BOOL: return "bool";
    case DT_INTROSPECTION_TYPE_ARRAY: return "array";
    case DT_INTROSPECTION_TYPE_ENUM: return "enum";
    case DT_INTROSPECTION_TYPE_STRUCT: return "struct";
    case DT_INTROSPECTION_TYPE_UNION: return "union";
    default: return "none";
  }
}

static void field(const dt_introspection_field_t *f)
{
  const dt_introspection_type_header_t *h = &f->header;
  printf("{\"name\":");
  json_string(h->field_name);
  printf(",\"type\":\"%s\",\"offset\":%zu,\"size\":%zu", type_name(h->type), h->offset, h->size);
  switch(h->type)
  {
    case DT_INTROSPECTION_TYPE_FLOAT:
      printf(",\"min\":%.9g,\"max\":%.9g,\"default\":%.9g", f->Float.Min, f->Float.Max, f->Float.Default);
      break;
    case DT_INTROSPECTION_TYPE_INT:
      printf(",\"min\":%d,\"max\":%d,\"default\":%d", f->Int.Min, f->Int.Max, f->Int.Default);
      break;
    case DT_INTROSPECTION_TYPE_UINT:
      printf(",\"min\":%u,\"max\":%u,\"default\":%u", f->UInt.Min, f->UInt.Max, f->UInt.Default);
      break;
    case DT_INTROSPECTION_TYPE_BOOL:
      printf(",\"default\":%s", f->Bool.Default ? "true" : "false");
      break;
    case DT_INTROSPECTION_TYPE_ENUM:
      printf(",\"default\":%d,\"values\":[", f->Enum.Default);
      // An anonymous enum (liquify's) carries no values: its fields are not set.
      for(size_t i = 0; h->type_name && *h->type_name && f->Enum.values && i < f->Enum.entries; i++)
      {
        printf(i ? ",{\"name\":" : "{\"name\":");
        json_string(f->Enum.values[i].name);
        printf(",\"value\":%d}", f->Enum.values[i].value);
      }
      printf("]");
      break;
    case DT_INTROSPECTION_TYPE_ARRAY:
      printf(",\"count\":%zu,\"element\":", f->Array.count);
      if(f->Array.field) field(f->Array.field);
      else printf("null");
      break;
    case DT_INTROSPECTION_TYPE_STRUCT:
    case DT_INTROSPECTION_TYPE_UNION:
      printf(",\"fields\":[");
      for(size_t i = 0; f->Struct.fields && i < f->Struct.entries && f->Struct.fields[i]; i++)
      {
        if(i) putchar(',');
        field(f->Struct.fields[i]);
      }
      printf("]");
      break;
    default:
      break;
  }
  putchar('}');
}

int main(int argc, char **argv)
{
  if(argc != 2)
  {
    fprintf(stderr, "usage: %s PLUGIN_DIR\n", argv[0]);
    return 2;
  }
  DIR *dir = opendir(argv[1]);
  if(!dir)
  {
    perror(argv[1]);
    return 1;
  }
  struct dirent *e;
  int first = 1;
  printf("{\"api_version\":%d,\"modules\":[\n", DT_INTROSPECTION_VERSION);
  while((e = readdir(dir)))
  {
    const size_t len = strlen(e->d_name);
    if(strncmp(e->d_name, "lib", 3) || len < 7 || strcmp(e->d_name + len - 3, ".so")) continue;
    char path[4096];
    snprintf(path, sizeof(path), "%s/%s", argv[1], e->d_name);
    if(getenv("DT_LAYOUTS_TRACE")) fprintf(stderr, "%s\n", e->d_name);
    void *lib = dlopen(path, RTLD_LAZY | RTLD_LOCAL);
    if(!lib)
    {
      fprintf(stderr, "skipped %s: %s\n", e->d_name, dlerror());
      continue;
    }
    init_fn init = (init_fn)dlsym(lib, "introspection_init");
    get_fn get = (get_fn)dlsym(lib, "get_introspection");
    if(!init || !get || init(NULL, DT_INTROSPECTION_VERSION))
    {
      dlclose(lib);
      continue;
    }
    const dt_introspection_t *in = get();
    fflush(stdout);
    char name[256];
    snprintf(name, sizeof(name), "%.*s", (int)(len - 6), e->d_name + 3);
    printf(first ? "{\"module\":" : ",\n{\"module\":");
    first = 0;
    json_string(name);
    printf(",\"version\":%d,\"type\":", in->params_version);
    json_string(in->type_name);
    printf(",\"size\":%zu,\"params\":", in->size);
    field(in->field);
    putchar('}');
  }
  printf("\n]}\n");
  closedir(dir);
  return 0;
}
