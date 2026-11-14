#!/usr/bin/env node
"use strict";

const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const https = require("node:https");
const os = require("node:os");
const path = require("node:path");

const REPO = "vefacaglar/locagens";

function downloadFile(url, destPath, redirectsLeft = 10) {
  return new Promise((resolve, reject) => {
    const req = https.get(
      url,
      { headers: { "User-Agent": "lcgn-npm-launcher" } },
      (res) => {
        const status = res.statusCode ?? 0;
        if (status >= 300 && status < 400 && res.headers.location) {
          res.resume();
          if (redirectsLeft <= 0) {
            reject(new Error(`Too many redirects while downloading ${url}`));
            return;
          }
          const nextUrl = new URL(res.headers.location, url).toString();
          downloadFile(nextUrl, destPath, redirectsLeft - 1).then(resolve, reject);
          return;
        }
        if (status !== 200) {
          res.resume();
          const err = new Error(`Download failed (${status}) for ${url}`);
          err.statusCode = status;
          reject(err);
          return;
        }
        const out = fs.createWriteStream(destPath);
        res.pipe(out);
        out.on("finish", () => out.close(resolve));
        out.on("error", reject);
      }
    );
    req.on("error", reject);
  });
}

function findExtractedBinary(dir, binName) {
  const direct = path.join(dir, binName);
  if (fs.existsSync(direct)) {
    return direct;
  }
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (entry.isDirectory()) {
      const found = findExtractedBinary(path.join(dir, entry.name), binName);
      if (found) {
        return found;
      }
    }
  }
  return null;
}

async function ensureCachedBinary(version) {
  const platform = process.platform;
  if (platform !== "darwin" && platform !== "win32") {
    throw new Error(
      `Unsupported platform for prebuilt lcgn binary: ${platform}-${process.arch}`
    );
  }

  const isWin = platform === "win32";
  const binName = isWin ? "lcgn.exe" : "lcgn";
  const archiveName = isWin ? "lcgn-Windows-x64.zip" : "lcgn-macOS-universal.tar.gz";
  const cacheDir = path.join(os.homedir(), ".locagens", "bin");
  const cachedBin = path.join(cacheDir, `lcgn-v${version}${isWin ? ".exe" : ""}`);

  if (fs.existsSync(cachedBin)) {
    return cachedBin;
  }

  fs.mkdirSync(cacheDir, { recursive: true });
  const tmpDir = fs.mkdtempSync(path.join(cacheDir, ".tmp-"));
  const archivePath = path.join(tmpDir, archiveName);
  const extractDir = path.join(tmpDir, "extract");
  const tmpBin = path.join(cacheDir, `.lcgn-v${version}-${process.pid}.tmp`);

  try {
    const taggedUrl = `https://github.com/${REPO}/releases/download/v${version}/${archiveName}`;
    const latestUrl = `https://github.com/${REPO}/releases/latest/download/${archiveName}`;
    console.error(`Downloading ${archiveName} (v${version})...`);
    try {
      await downloadFile(taggedUrl, archivePath);
    } catch (err) {
      if (err.statusCode !== 404) {
        throw err;
      }
      await downloadFile(latestUrl, archivePath);
    }

    fs.mkdirSync(extractDir, { recursive: true });
    const tarArgs = isWin
      ? ["-xf", archivePath, "-C", extractDir]
      : ["-xzf", archivePath, "-C", extractDir];
    const tarResult = spawnSync("tar", tarArgs, { stdio: "inherit" });
    if (tarResult.error) {
      throw tarResult.error;
    }
    if (tarResult.status !== 0) {
      throw new Error(`tar exited with code ${tarResult.status}`);
    }

    const extracted = findExtractedBinary(extractDir, binName);
    if (!extracted) {
      throw new Error(`Archive ${archiveName} did not contain ${binName}`);
    }

    fs.copyFileSync(extracted, tmpBin);
    fs.chmodSync(tmpBin, 0o755);
    fs.renameSync(tmpBin, cachedBin);
    return cachedBin;
  } finally {
    fs.rmSync(tmpBin, { force: true });
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
}

async function main() {
  const { version } = require("../package.json");
  if (process.argv[2] === "--install-only") {
    try {
      await ensureCachedBinary(version);
    } catch (err) {
      console.warn(
        `Warning: could not pre-download lcgn (${err.message}). It will download on first run.`
      );
    }
    process.exit(0);
  }

  const binPath = await ensureCachedBinary(version);
  const result = spawnSync(binPath, process.argv.slice(2), { stdio: "inherit" });
  if (result.error) {
    console.error(`Failed to launch lcgn: ${result.error.message}`);
    process.exit(1);
  }
  process.exit(result.status ?? 1);
}

main().catch((err) => {
  console.error(`lcgn launcher error: ${err.message}`);
  process.exit(1);
});
