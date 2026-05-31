#!/usr/bin/env bash
set -euo pipefail

# Configuration
REPO="JanosMozer/researcher-agent-kube-environment"
BINARY_NAME="raket-controller"

echo "Querying GitHub API for the latest release..."
LATEST_TAG=$(curl -s "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name":' | sed -E 's/.*"([^"]+)".*/\1/')

if [ -z "${LATEST_TAG}" ]; then
  echo "Error: Could not retrieve the latest release version tag."
  exit 1
fi

echo "Installing ${BINARY_NAME} (${LATEST_TAG})..."

# Detect platform and architecture
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)

case "${OS}" in
  darwin)  PLATFORM="apple-darwin" ;;
  linux)   PLATFORM="unknown-linux-gnu" ;;
  *)       echo "Error: Unsupported Operating System: ${OS}"; exit 1 ;;
esac

case "${ARCH}" in
  x86_64)  TARGET_ARCH="x86_64" ;;
  arm64|aarch64) TARGET_ARCH="aarch64" ;;
  *)       echo "Error: Unsupported CPU Architecture: ${ARCH}"; exit 1 ;;
esac

ASSET_NAME="${BINARY_NAME}-${TARGET_ARCH}-${PLATFORM}.tar.gz"
DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${LATEST_TAG}/${ASSET_NAME}"

echo "Downloading archive from ${DOWNLOAD_URL}..."
if ! curl -L -f "${DOWNLOAD_URL}" -o "${ASSET_NAME}"; then
  echo "Error: Failed to download release asset."
  exit 1
fi

echo "Extracting binary..."
tar -xzf "${ASSET_NAME}"
chmod +x "${BINARY_NAME}"

mkdir -p ./bin
mv "${BINARY_NAME}" ./bin/
rm "${ASSET_NAME}"

echo "Installation complete!"
echo "The binary has been placed in: ./bin/${BINARY_NAME}"
echo "Run it with: ./bin/${BINARY_NAME} --help"
