#include <errno.h>
#include <fcntl.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

static int invalid_component(const char *component) {
    return strcmp(component, ".") == 0 || strcmp(component, "..") == 0;
}

// Every directory is opened relative to its already-open parent. Holding the
// descriptor pins that directory even if another process renames or replaces
// a path component. No symlink is followed, including the attachment root.
int messages_open_attachment(const char *root, const char *relative) {
    if (root[0] != '/' || relative[0] == '/' || relative[0] == '\0') {
        errno = EINVAL;
        return -1;
    }
    char *root_copy = strdup(root);
    char *relative_copy = strdup(relative);
    int directory = -1;
    int file = -1;
    if (root_copy == NULL || relative_copy == NULL) {
        errno = ENOMEM;
        goto cleanup;
    }
    directory = open("/", O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
    if (directory < 0) {
        goto cleanup;
    }
    char *state = NULL;
    for (char *component = strtok_r(root_copy, "/", &state); component != NULL; component = strtok_r(NULL, "/", &state)) {
        if (invalid_component(component)) {
            errno = EINVAL;
            goto cleanup;
        }
        int next = openat(directory, component, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
        if (next < 0) {
            goto cleanup;
        }
        close(directory);
        directory = next;
    }
    state = NULL;
    char *component = strtok_r(relative_copy, "/", &state);
    if (component == NULL) {
        errno = EINVAL;
        goto cleanup;
    }
    for (;;) {
        if (invalid_component(component)) {
            errno = EINVAL;
            goto cleanup;
        }
        char *next_component = strtok_r(NULL, "/", &state);
        if (next_component == NULL) {
            // O_NONBLOCK prevents FIFO/device opens from waiting before fstat
            // can reject them. No bytes are read until regularity is verified.
            file = openat(directory, component, O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC);
            if (file < 0) {
                goto cleanup;
            }
            struct stat metadata;
            if (fstat(file, &metadata) != 0) {
                goto failure;
            }
            if (!S_ISREG(metadata.st_mode)) {
                errno = EINVAL;
                goto failure;
            }
            break;
        }
        int next = openat(directory, component, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC);
        if (next < 0) {
            goto cleanup;
        }
        close(directory);
        directory = next;
        component = next_component;
    }
    goto cleanup;

failure: {
        int saved_errno = errno;
        close(file);
        file = -1;
        errno = saved_errno;
    }
cleanup: {
        int saved_errno = errno;
        if (directory >= 0) {
            close(directory);
        }
        free(root_copy);
        free(relative_copy);
        errno = saved_errno;
    }
    return file;
}
