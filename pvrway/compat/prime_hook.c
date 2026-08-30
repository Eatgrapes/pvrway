#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>
#include <xf86drmMode.h>
#include <xf86drm.h>

struct fake_buffer {
    uint32_t handle;
    int fd;
    size_t size;
};

typedef int ion_user_handle_t;
struct ion_allocation_data {
    size_t len;
    size_t align;
    unsigned int heap_id_mask;
    unsigned int flags;
    ion_user_handle_t handle;
};
struct ion_fd_data {
    ion_user_handle_t handle;
    int fd;
};
struct ion_handle_data {
    ion_user_handle_t handle;
};
#define ION_IOC_MAGIC 'I'
#define ION_IOC_ALLOC _IOWR(ION_IOC_MAGIC, 0, struct ion_allocation_data)
#define ION_IOC_FREE _IOWR(ION_IOC_MAGIC, 1, struct ion_handle_data)
#define ION_IOC_SHARE _IOWR(ION_IOC_MAGIC, 4, struct ion_fd_data)

static struct fake_buffer buffers[256];
static uint32_t next_handle = 1;

static struct fake_buffer *find_buffer(uint32_t handle) {
    if (!handle)
        return NULL;
    for (size_t i = 0; i < 256; ++i)
        if (buffers[i].handle == handle)
            return &buffers[i];
    return NULL;
}

static struct fake_buffer *create_buffer(size_t size) {
    for (size_t i = 0; i < 256; ++i) {
        if (buffers[i].handle)
            continue;
        int ion = open("/dev/ion", O_RDWR | O_CLOEXEC);
        struct ion_allocation_data allocation = {
            .len = size,
            .align = 4096,
            .heap_id_mask = 1,
        };
        if (ion < 0 || ioctl(ion, ION_IOC_ALLOC, &allocation) < 0) {
            if (ion >= 0)
                close(ion);
            return NULL;
        }
        struct ion_fd_data share = {.handle = allocation.handle, .fd = -1};
        int shared = ioctl(ion, ION_IOC_SHARE, &share);
        struct ion_handle_data free_data = {.handle = allocation.handle};
        ioctl(ion, ION_IOC_FREE, &free_data);
        close(ion);
        if (shared < 0 || share.fd < 0)
            return NULL;
        buffers[i] = (struct fake_buffer){
            .handle = next_handle++,
            .fd = share.fd,
            .size = size,
        };
        return &buffers[i];
    }
    return NULL;
}

static struct fake_buffer *find_buffer_fd(int fd) {
    struct stat candidate;
    if (fstat(fd, &candidate) < 0)
        return NULL;
    for (size_t i = 0; i < 256; ++i) {
        struct stat stored;
        if (!buffers[i].handle || fstat(buffers[i].fd, &stored) < 0)
            continue;
        if (candidate.st_dev == stored.st_dev && candidate.st_ino == stored.st_ino)
            return &buffers[i];
    }
    return NULL;
}

int drmGetCap(int fd, uint64_t capability, uint64_t *value) {
    static int (*real_drmGetCap)(int, uint64_t, uint64_t *);
    if (!real_drmGetCap)
        real_drmGetCap = dlsym(RTLD_NEXT, "drmGetCap");
    if (capability == 0x5) {
        *value = 0x3;
        return 0;
    }
    return real_drmGetCap(fd, capability, value);
}

int drmGetDeviceFromDevId(dev_t dev_id, uint32_t flags, drmDevicePtr *device) {
    static int (*real_get_device)(dev_t, uint32_t, drmDevicePtr *);
    if (!real_get_device)
        real_get_device = dlsym(RTLD_NEXT, "drmGetDeviceFromDevId");
    int result = real_get_device(dev_id, flags, device);
    if (result == 0 && *device)
        (*device)->available_nodes &= ~(1 << DRM_NODE_RENDER);
    return result;
}

int drmPrimeFDToHandle(int fd, int prime_fd, uint32_t *handle) {
    struct fake_buffer *buffer = find_buffer_fd(prime_fd);
    if (!buffer) {
        errno = ENOENT;
        return -1;
    }
    *handle = buffer->handle;
    return 0;
}

int drmPrimeHandleToFD(int fd, uint32_t handle, uint32_t flags, int *prime_fd) {
    struct fake_buffer *buffer = find_buffer(handle);
    if (!buffer) {
        errno = ENOENT;
        return -1;
    }
    *prime_fd = dup(buffer->fd);
    return *prime_fd < 0 ? -1 : 0;
}

int ioctl(int fd, unsigned long request, ...) {
    static int (*real_ioctl)(int, unsigned long, void *);
    if (!real_ioctl)
        real_ioctl = dlsym(RTLD_NEXT, "ioctl");
    va_list args;
    va_start(args, request);
    void *arg = va_arg(args, void *);
    va_end(args);

    if (request == DRM_IOCTL_MODE_CREATE_DUMB) {
        struct drm_mode_create_dumb *create = arg;
        create->pitch = create->width * ((create->bpp + 7) / 8);
        create->size = (uint64_t)create->pitch * create->height;
        struct fake_buffer *buffer = create_buffer(create->size);
        if (!buffer) {
            errno = ENOMEM;
            return -1;
        }
        create->handle = buffer->handle;
        return 0;
    }
    if (request == DRM_IOCTL_MODE_MAP_DUMB) {
        struct drm_mode_map_dumb *map = arg;
        if (!find_buffer(map->handle)) {
            errno = ENOENT;
            return -1;
        }
        map->offset = (uint64_t)map->handle << 32;
        return 0;
    }
    if (request == DRM_IOCTL_MODE_DESTROY_DUMB) {
        struct drm_mode_destroy_dumb *destroy = arg;
        struct fake_buffer *buffer = find_buffer(destroy->handle);
        if (buffer) {
            close(buffer->fd);
            *buffer = (struct fake_buffer){0};
        }
        return 0;
    }
    if (request == DRM_IOCTL_PRIME_HANDLE_TO_FD) {
        struct drm_prime_handle *prime = arg;
        struct fake_buffer *buffer = find_buffer(prime->handle);
        if (!buffer) {
            errno = ENOENT;
            return -1;
        }
        prime->fd = dup(buffer->fd);
        return prime->fd < 0 ? -1 : 0;
    }
    if (request == DRM_IOCTL_PRIME_FD_TO_HANDLE) {
        struct drm_prime_handle *prime = arg;
        struct fake_buffer *buffer = find_buffer_fd(prime->fd);
        if (!buffer) {
            errno = ENOENT;
            return -1;
        }
        prime->handle = buffer->handle;
        return 0;
    }
    if (request == DRM_IOCTL_GEM_CLOSE) {
        struct drm_gem_close *close_data = arg;
        if (find_buffer(close_data->handle))
            return 0;
    }
    return real_ioctl(fd, request, arg);
}

void *mmap(void *addr, size_t length, int prot, int flags, int fd, off_t offset) {
    static void *(*real_mmap)(void *, size_t, int, int, int, off_t);
    if (!real_mmap)
        real_mmap = dlsym(RTLD_NEXT, "mmap");
    uint32_t handle = (uint64_t)offset >> 32;
    struct fake_buffer *buffer = find_buffer(handle);
    if (buffer)
        return real_mmap(addr, length, prot, flags, buffer->fd, 0);
    return real_mmap(addr, length, prot, flags, fd, offset);
}
