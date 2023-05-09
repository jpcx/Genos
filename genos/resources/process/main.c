
#include <stdio.h>
#include <string.h>
#include <assert.h>
#include <stdlib.h>
#include <unistd.h>

int main(int argc, char **args) {
    if (argc == 1) {
        return 0;
    }
    const char *const arg = args[1];
    if (strcmp("segfault", arg) == 0) {
        int *n = NULL;
        int i = *n;
    } else if (strcmp("abort", arg) == 0) {
        abort();
    } else if (strcmp("timeout", arg) == 0) {
        sleep(3);
    } else if (strcmp("usersig", arg) == 0) {
        // not sure what this is doing
    } else if (strcmp("rc", arg) == 0) {
        assert(argc == 3);
        const char *rc_str = args[2];
        int v = atoi(rc_str);
        exit(v);
    } else if (strcmp("stderr", arg) == 0) {
        assert(argc == 3);
        const char *msg = args[2];
        fprintf(stderr, "%s\n", msg);
        fflush(stderr);
    } else if (strcmp("stdouterr", arg) == 0) {
        assert(argc == 3);
        const char *msg = args[2];
        fprintf(stdout, "OUT: %s\n", msg);
        fflush(stdout);
        fprintf(stderr, "ERR: %s\n", msg);
        fflush(stderr);
    }else if (strcmp("read_line_from_stdin", arg) == 0) {
        char *line = NULL;
        size_t line_size;
        getline(&line, &line_size, stdin);
        fprintf(stdout, "%s", line);
        fflush(stdout);
    }
    return 0;
}
