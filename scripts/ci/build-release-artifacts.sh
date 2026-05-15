#!/usr/bin/env sh
set -eu

export PATH="/usr/local/cargo/bin:$HOME/.cargo/bin:$PATH"

out_dir="${RELEASE_OUT_DIR:-target/release-upload}"
docker_dir="${RELEASE_DOCKER_DIR:-.docker-release}"
env_file="$out_dir/release.env"
if [ -f "$env_file" ]; then
    # shellcheck disable=SC1090
    . "$env_file"
fi

version="${RELEASE_VERSION:-$(
    awk '
        /^\[package\]/ { in_package = 1; next }
        /^\[/ { in_package = 0 }
        in_package && $1 == "version" {
            gsub(/"/, "", $3)
            print $3
            exit
        }
    ' Cargo.toml
)}"
tag="${RELEASE_TAG:-v$version}"
toolchain="${RUST_TOOLCHAIN:-$(awk -F '"' '/^channel/ { print $2; exit }' rust-toolchain.toml)}"
linux_target="x86_64-unknown-linux-gnu"
windows_target="x86_64-pc-windows-gnu"
repo_dir="$(pwd)"

if command -v rustup >/dev/null 2>&1; then
    rustup target add --toolchain "$toolchain" "$linux_target" "$windows_target"
fi

cargo "+$toolchain" build --release --locked --bin sendword --target "$linux_target"
cargo "+$toolchain" build --release --locked --bin sendword --target "$windows_target"

rm -rf "$out_dir"
mkdir -p "$out_dir"
rm -rf "$docker_dir"
mkdir -p "$docker_dir"
cp "target/$linux_target/release/sendword" "$docker_dir/sendword"
chmod 0755 "$docker_dir/sendword"

work_dir="$(mktemp -d)"
trap 'rm -rf "$work_dir"' EXIT

package_dir="$work_dir/sendword"
mkdir -p "$package_dir"
cp "target/$linux_target/release/sendword" "$package_dir/sendword"
cp LICENSE README.md "$package_dir/"
tar -C "$work_dir" -czf "$out_dir/sendword-$linux_target.tar.gz" sendword

rm -rf "$package_dir"
mkdir -p "$package_dir"
cp "target/$windows_target/release/sendword.exe" "$package_dir/sendword.exe"
cp LICENSE README.md "$package_dir/"
(cd "$work_dir" && zip -qr "$repo_dir/$out_dir/sendword-$windows_target.zip" sendword)

cat > "$out_dir/sendword-installer.sh" <<'INSTALL_SH'
#!/usr/bin/env sh
set -eu

version="${SENDWORD_VERSION:-latest}"
base_url="${SENDWORD_INSTALL_BASE_URL:-https://releases.sendword.online/$version}"
install_dir="${SENDWORD_INSTALL_DIR:-$HOME/.cargo/bin}"
archive="sendword-x86_64-unknown-linux-gnu.tar.gz"

case "$(uname -s)" in
    Linux) ;;
    *)
        echo "Unsupported OS: $(uname -s). Use the PowerShell installer on Windows." >&2
        exit 1
        ;;
esac

case "$(uname -m)" in
    x86_64|amd64) ;;
    *)
        echo "Unsupported architecture: $(uname -m). Only x86_64 Linux is published." >&2
        exit 1
        ;;
esac

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

url="$base_url/$archive"
if command -v curl >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -fsSL "$url" -o "$tmp_dir/$archive"
elif command -v wget >/dev/null 2>&1; then
    wget -q "$url" -O "$tmp_dir/$archive"
else
    echo "curl or wget is required to install sendword" >&2
    exit 1
fi

tar -xzf "$tmp_dir/$archive" -C "$tmp_dir"
mkdir -p "$install_dir"
if command -v install >/dev/null 2>&1; then
    install -m 0755 "$tmp_dir/sendword/sendword" "$install_dir/sendword"
else
    cp "$tmp_dir/sendword/sendword" "$install_dir/sendword"
    chmod 0755 "$install_dir/sendword"
fi

echo "sendword installed to $install_dir/sendword"
INSTALL_SH

cat > "$out_dir/sendword-installer.ps1" <<'INSTALL_PS1'
$ErrorActionPreference = "Stop"

$Version = if ($env:SENDWORD_VERSION) { $env:SENDWORD_VERSION } else { "latest" }
$BaseUrl = if ($env:SENDWORD_INSTALL_BASE_URL) {
    $env:SENDWORD_INSTALL_BASE_URL.TrimEnd("/")
} else {
    "https://releases.sendword.online/$Version"
}
$InstallDir = if ($env:SENDWORD_INSTALL_DIR) {
    $env:SENDWORD_INSTALL_DIR
} else {
    Join-Path $HOME ".cargo\bin"
}
$Archive = "sendword-x86_64-pc-windows-gnu.zip"
$TempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("sendword-install-" + [System.Guid]::NewGuid().ToString("N"))

if (-not [Environment]::Is64BitOperatingSystem) {
    throw "Only 64-bit Windows is supported by this installer."
}

New-Item -ItemType Directory -Force -Path $TempDir | Out-Null
try {
    $ZipPath = Join-Path $TempDir $Archive
    Invoke-WebRequest -Uri "$BaseUrl/$Archive" -OutFile $ZipPath
    Expand-Archive -Path $ZipPath -DestinationPath $TempDir -Force
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item -Path (Join-Path $TempDir "sendword\sendword.exe") -Destination (Join-Path $InstallDir "sendword.exe") -Force
    Write-Host "sendword installed to $(Join-Path $InstallDir "sendword.exe")"
} finally {
    Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue
}
INSTALL_PS1

chmod 0755 "$out_dir/sendword-installer.sh"

{
    echo "RELEASE_TAG=$tag"
    echo "RELEASE_VERSION=$version"
    echo "R2_BUCKET=${R2_BUCKET:-sendword-releases}"
    echo "R2_PUBLIC_BASE_URL=${R2_PUBLIC_BASE_URL:-https://releases.sendword.online}"
} > "$env_file"

(cd "$out_dir" && sha256sum \
    "sendword-$linux_target.tar.gz" \
    "sendword-$windows_target.zip" \
    sendword-installer.sh \
    sendword-installer.ps1 > SHA256SUMS)

echo "Built release artifacts for $tag in $out_dir"
