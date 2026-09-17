version := `awk -F '"' '/^version = "/ { print $2; exit }' Cargo.toml`

test:
    cargo test

# Cross-compile the versioned Windows executable from macOS.
build:
    RC=x86_64-w64-mingw32-windres cargo xwin build --release --target x86_64-pc-windows-msvc
    mkdir -p dist
    cp target/x86_64-pc-windows-msvc/release/vrchat-x3d-start-fix.exe dist/vrchat-x3d-start-fix-v{{version}}-windows-x64.exe
    @echo "dist/vrchat-x3d-start-fix-v{{version}}-windows-x64.exe"

windows: build

# Bump the package version. Defaults to patch; accepts minor, major, or X.Y.Z.
up-version part="patch":
    bash scripts/up-version.sh {{part}}
