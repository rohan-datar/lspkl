# Requires Just >= 1.32, Bash (Git Bash on Windows), Cargo, jq, file, tar, zip/unzip,
# cmp, find, sort, mktemp, and sha256sum or shasum. Linux also needs readelf;
# Linux builds need cross and Docker. Native smoke builds the Rust smoke example.
set positional-arguments

# Print the Cargo package version; an optional tag must be exactly vVERSION.
[script("bash")]
release-version tag="":
    set -euo pipefail
    version=$(cargo metadata --no-deps --format-version 1 --locked | jq -er '.packages[] | select(.name == "lspkl") | .version')
    if [[ -n "$1" && "$1" != "v$version" ]]; then
        echo "Release tag $1 does not match Cargo version v$version" >&2
        exit 1
    fi
    printf '%s\n' "$version"

# Build one supported release target (cross on Linux, Cargo on macOS/Windows).
[script("bash")]
release-build target:
    set -euo pipefail
    case "$1" in
        x86_64-unknown-linux-musl|aarch64-unknown-linux-musl)
            cross build --release --locked -p lspkl --bin lspkl --target "$1" ;;
        aarch64-apple-darwin|x86_64-apple-darwin|x86_64-pc-windows-msvc)
            cargo build --release --locked -p lspkl --bin lspkl --target "$1" ;;
        *) echo "Unsupported release target: $1" >&2; exit 1 ;;
    esac

# Reject wrong CPU/OS labels and dynamically linked Linux artifacts.
[script("bash")]
release-validate-target binary target:
    set -euo pipefail
    description=$(LC_ALL=C file -b "$1")
    trap 'echo "Binary does not match $2: $description" >&2' ERR
    case "$2" in
        x86_64-unknown-linux-musl) [[ "$description" == *"ELF 64-bit LSB"* && "$description" == *"x86-64"* ]]; os=linux ;;
        aarch64-unknown-linux-musl) [[ "$description" == *"ELF 64-bit LSB"* && "$description" == *"ARM aarch64"* ]]; os=linux ;;
        x86_64-apple-darwin) [[ "$description" == *"Mach-O 64-bit"* && "$description" == *"x86_64"* ]]; os=darwin ;;
        aarch64-apple-darwin) [[ "$description" == *"Mach-O 64-bit"* && "$description" == *"arm64"* ]]; os=darwin ;;
        x86_64-pc-windows-msvc) [[ "$description" == *"PE32+ executable"* && "$description" == *"x86-64"* && "$description" == *"for MS Windows"* ]]; os=windows ;;
        *) echo "Unsupported release target: $2" >&2; exit 1 ;;
    esac
    if [[ "$os" == linux ]]; then
        if [[ "$description" != *"statically linked"* && "$description" != *"static-pie linked"* ]]; then
            echo "Refusing non-static musl artifact: $description" >&2
            exit 1
        fi
        headers=$(LC_ALL=C readelf -l "$1")
        if [[ "$headers" == *INTERP* ]]; then
            echo "Refusing musl artifact with ELF PT_INTERP" >&2
            exit 1
        fi
    fi
    printf 'Validated %s: %s\n' "$2" "$description"

# Package, unpack, compare every file, then run the real unpacked binary if native.
[script("bash")]
release-package target binary="" tag="":
    set -euo pipefail
    target=$1
    binary=$2
    version=$(just release-version "$3")
    executable=lspkl
    [[ "$target" != x86_64-pc-windows-msvc ]] || executable=lspkl.exe
    [[ -n "$binary" ]] || binary="target/$target/release/$executable"
    just release-validate-target "$binary" "$target"
    output="$PWD/target/release-artifacts"
    mkdir -p "$output"
    temporary=$(mktemp -d "$output/.package.XXXXXXXX")
    trap 'rm -rf "$temporary"' EXIT
    stem="lspkl-$version-$target"
    staging="$temporary/$stem"
    mkdir -p "$staging/licenses/pkl-stdlib" "$temporary/unpacked"
    cp "$binary" "$staging/$executable"
    chmod 755 "$staging/$executable"
    cp LICENSE NOTICE.txt "$staging/"
    cp crates/pkl-stdlib/LICENSE crates/pkl-stdlib/NOTICE "$staging/licenses/pkl-stdlib/"
    if [[ "$executable" == *.exe ]]; then
        archive="$temporary/$stem.zip"
        (cd "$temporary"; zip -q -r "$archive" "$stem")
        unzip -q "$archive" -d "$temporary/unpacked"
    else
        archive="$temporary/$stem.tar.gz"
        tar -czf "$archive" -C "$temporary" "$stem"
        tar -xzf "$archive" -C "$temporary/unpacked"
    fi
    contents="$temporary/unpacked/$stem"
    printf '%s\n' "$executable" LICENSE NOTICE.txt licenses/pkl-stdlib/LICENSE licenses/pkl-stdlib/NOTICE | LC_ALL=C sort > "$temporary/expected"
    (cd "$contents"; find . -type f | while IFS= read -r name; do printf '%s\n' "${name#./}"; done | LC_ALL=C sort) > "$temporary/actual"
    cmp "$temporary/expected" "$temporary/actual"
    while IFS= read -r name; do cmp "$staging/$name" "$contents/$name"; done < "$temporary/expected"
    just release-validate-target "$contents/$executable" "$target"
    machine=$(uname -m)
    case "$machine" in arm64) machine=aarch64 ;; AMD64|amd64) machine=x86_64 ;; esac
    platform=$(uname -s)
    case "$platform" in Linux) host="$machine-unknown-linux-musl" ;; Darwin) host="$machine-apple-darwin" ;; MINGW*|MSYS*|CYGWIN*) host="$machine-pc-windows-msvc" ;; *) host=unsupported ;; esac
    if [[ "$host" == "$target" ]]; then
        actual_version=$("$contents/$executable" --version)
        [[ "$actual_version" == "lspkl $version" ]]
        "$contents/$executable" --help
        # Windows Cargo needs a Windows-native path, not Git Bash's /c/... path.
        smoke_binary="$contents/$executable"
        if [[ "$platform" == MINGW* || "$platform" == MSYS* || "$platform" == CYGWIN* ]]; then
            smoke_binary=$(cygpath -w "$smoke_binary")
        fi
        cargo run --locked -p lspkl --example smoke -- --binary "$smoke_binary"
        echo 'PASS: unpacked --version, --help and real stdio smoke'
    else
        echo "NOT RUN: $target is off-platform/off-architecture on $platform/$machine; archive contents validated only"
    fi
    mv "$archive" "$output/"
    just release-checksums
    echo "Packaged $output/$(basename "$archive")"

# Recompute aggregate SHA256SUMS from all release archives (also used at publish).
[script("bash")]
release-checksums directory="target/release-artifacts":
    set -euo pipefail
    cd "$1"
    shopt -s nullglob
    archives=(*.tar.gz *.zip)
    [[ ${#archives[@]} -gt 0 ]] || { echo 'No release archives found' >&2; exit 1; }
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "${archives[@]}" | LC_ALL=C sort -k2 > SHA256SUMS
    else
        shasum -a 256 "${archives[@]}" | LC_ALL=C sort -k2 > SHA256SUMS
    fi
    echo "Wrote SHA256SUMS for ${#archives[@]} archive(s)"
