#include <blake2.h>
#include <lz4.h>
#include <string.h>

int main(void) {
    const char input[] = "RustZIP macOS library bundling smoke test";
    unsigned char digest[64];
    char compressed[LZ4_COMPRESSBOUND(sizeof(input))];

    if (blake2b(digest, input, NULL, sizeof(digest), strlen(input), 0) != 0) {
        return 1;
    }
    if (LZ4_compress_default(input, compressed, sizeof(input), sizeof(compressed)) <= 0) {
        return 2;
    }
    return 0;
}
