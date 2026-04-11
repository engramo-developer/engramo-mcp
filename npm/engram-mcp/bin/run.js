#!/usr/bin/env node

'use strict';

const os = require('os');
const path = require('path');
const fs = require('fs');
const { spawnSync } = require('child_process');

const PLATFORM_PACKAGES = {
  'darwin-arm64': '@engram-fc/mcp-darwin-arm64',
  'darwin-x64': '@engram-fc/mcp-darwin-x64',
  'linux-arm64': '@engram-fc/mcp-linux-arm64',
  'linux-x64': '@engram-fc/mcp-linux-x64',
};

function getPlatformKey() {
  const platform = os.platform();
  const arch = os.arch();

  const archMap = { x64: 'x64', arm64: 'arm64' };
  const mappedArch = archMap[arch];
  if (!mappedArch) return null;

  return `${platform}-${mappedArch}`;
}

function resolveBinaryPath(pkg) {
  try {
    const pkgJsonPath = require.resolve(`${pkg}/package.json`);
    const pkgDir = path.dirname(pkgJsonPath);
    return path.join(pkgDir, 'engram-mcp');
  } catch {
    return null;
  }
}

const platformKey = getPlatformKey();
const pkg = platformKey ? PLATFORM_PACKAGES[platformKey] : null;

if (!pkg) {
  const supported = Object.keys(PLATFORM_PACKAGES).join(', ');
  process.stderr.write(
    `engram-mcp: unsupported platform: ${os.platform()}-${os.arch()}\n` +
    `Supported platforms: ${supported}\n` +
    `Build from source: https://github.com/volmyrdot/engram-mcp\n`
  );
  process.exit(1);
}

const binPath = resolveBinaryPath(pkg);

if (!binPath) {
  process.stderr.write(
    `engram-mcp: could not find binary for ${platformKey}.\n` +
    `The optional dependency '${pkg}' may not have been installed.\n` +
    `Try: npm install ${pkg}\n`
  );
  process.exit(1);
}

// Ensure binary is executable
try {
  fs.chmodSync(binPath, 0o755);
} catch {
  // Ignore chmod errors (e.g. on read-only filesystems)
}

const result = spawnSync(binPath, process.argv.slice(2), { stdio: 'inherit' });

if (result.error) {
  process.stderr.write(`engram-mcp: failed to start binary: ${result.error.message}\n`);
  process.exit(1);
}

process.exit(result.status ?? 1);
