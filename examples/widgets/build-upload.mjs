#!/usr/bin/env node
// SPDX-License-Identifier: MPL-2.0

// Build a Panelet STORE-only .pnw archive. stdout is deliberately the raw
// binary package so it can be sent with curl --data-binary; diagnostics stay on
// stderr and never corrupt the package stream.

import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { TextDecoder } from "node:util";
import { fileURLToPath } from "node:url";

const PACKAGE_MAX = 64 * 1024;
const MANIFEST_MAX = 8 * 1024;
const SOURCE_MAX = 5 * 1024;
const RESOURCE_COUNT_MAX = 8;
const RESOURCE_ITEM_MAX = 1024;
const RESOURCE_TOTAL_MAX = 4 * 1024;
const ENTRY_COUNT_MAX = RESOURCE_COUNT_MAX + 2;
const RESOURCE_PATH_MAX = 64;

function fail(message) {
  console.error(`build-upload: ${message}`);
  process.exit(1);
}

const utf8 = new TextDecoder("utf-8", { fatal: true });

function readUtf8(file) {
  try {
    return utf8.decode(fs.readFileSync(file));
  } catch (error) {
    fail(`${path.relative(root, file)} is not valid UTF-8: ${error.message}`);
  }
}

function normalizeResourcePath(resourcePath) {
  if (
    resourcePath.length === 0 ||
    Buffer.byteLength(resourcePath) > RESOURCE_PATH_MAX ||
    resourcePath.startsWith("/") ||
    resourcePath.endsWith("/") ||
    resourcePath.includes("\\") ||
    resourcePath.includes(":") ||
    [...resourcePath].some(character => {
      const codePoint = character.codePointAt(0);
      return codePoint < 0x20 || (codePoint >= 0x7f && codePoint <= 0x9f);
    })
  ) {
    fail(`invalid resource path: ${resourcePath}`);
  }
  const segments = resourcePath.split("/");
  if (segments.some(segment => segment.length === 0 || segment === "." || segment === "..")) {
    fail(`invalid resource path: ${resourcePath}`);
  }
  return resourcePath;
}

function collectFiles(directory, prefix = "") {
  let directoryStat;
  try {
    directoryStat = fs.lstatSync(directory);
  } catch (error) {
    if (error.code === "ENOENT" && prefix === "") return [];
    fail(`${path.relative(root, directory)} cannot be read: ${error.message}`);
  }
  if (directoryStat.isSymbolicLink()) fail(`${path.relative(root, directory)} must not be a symlink`);
  if (!directoryStat.isDirectory()) fail(`${path.relative(root, directory)} must be a directory`);
  return fs.readdirSync(directory, { withFileTypes: true })
    .sort((left, right) => left.name < right.name ? -1 : left.name > right.name ? 1 : 0)
    .flatMap(entry => {
      const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
      const absolute = path.join(directory, entry.name);
      if (entry.isSymbolicLink()) fail(`${path.relative(root, absolute)} must not be a symlink`);
      if (entry.isDirectory()) return collectFiles(absolute, relative);
      if (!entry.isFile()) fail(`${path.relative(root, absolute)} must be a regular file`);
      return [{ path: relative.split(path.sep).join("/"), absolute }];
    });
}

const name = process.argv[2];
if (!name || !/^[a-z0-9][a-z0-9-]*$/.test(name)) {
  fail("usage: node examples/widgets/build-upload.mjs <widget-directory>");
}

const root = path.dirname(fileURLToPath(import.meta.url));
const widget = path.join(root, name);
const manifestPath = path.join(widget, "manifest.json");
const sourcePath = path.join(widget, "main.rn");
const resourcesRoot = path.join(widget, "resources");

if (!fs.existsSync(manifestPath) || !fs.existsSync(sourcePath)) {
  fail(`${name} must contain manifest.json and main.rn`);
}
if (!fs.lstatSync(manifestPath).isFile() || !fs.lstatSync(sourcePath).isFile()) {
  fail(`${name}/manifest.json and ${name}/main.rn must be regular files`);
}

let manifest;
try {
  manifest = JSON.parse(readUtf8(manifestPath));
} catch (error) {
  fail(`cannot parse ${path.relative(root, manifestPath)}: ${error.message}`);
}

const manifestFields = ["id", "name", "version", "abi", "capabilities", "host_permissions"];
if (!manifest || Array.isArray(manifest) || typeof manifest !== "object") {
  fail("manifest.json must contain one manifest object");
}
const unknownManifestFields = Object.keys(manifest).filter(field => !manifestFields.includes(field));
if (unknownManifestFields.length) {
  fail(`manifest.json has unknown fields: ${unknownManifestFields.join(", ")}`);
}
for (const field of ["id", "name", "version", "abi"]) {
  if (typeof manifest[field] !== "string" || !manifest[field]) fail(`manifest.${field} must be a non-empty string`);
}
if (manifest.abi !== "note4c.rune/3") fail("unsupported widget ABI; expected note4c.rune/3");
for (const field of ["capabilities", "host_permissions"]) {
  if (!Array.isArray(manifest[field]) || manifest[field].some(value => typeof value !== "string")) {
    fail(`manifest.${field} must be an array of strings`);
  }
}

const source = readUtf8(sourcePath);
const sourceBytes = Buffer.byteLength(source);
if (sourceBytes === 0 || sourceBytes > SOURCE_MAX) {
  fail(`source is ${sourceBytes} bytes; expected 1..${SOURCE_MAX}`);
}

const manifestBytes = Buffer.from(JSON.stringify(manifest), "utf8");
if (manifestBytes.length > MANIFEST_MAX) {
  fail(`manifest.json is ${manifestBytes.length} bytes; maximum is ${MANIFEST_MAX}`);
}

const resourceFiles = collectFiles(resourcesRoot);
if (resourceFiles.length > RESOURCE_COUNT_MAX) {
  fail(`resource count is ${resourceFiles.length}; maximum is ${RESOURCE_COUNT_MAX}`);
}

let resourceBytes = 0;
const resourcePaths = new Set();
const resources = resourceFiles.map(resource => {
  const resourcePath = normalizeResourcePath(resource.path);
  if (resourcePaths.has(resourcePath)) fail(`duplicate resource path: ${resourcePath}`);
  resourcePaths.add(resourcePath);
  const text = readUtf8(resource.absolute);
  const textBytes = Buffer.byteLength(text);
  if (textBytes > RESOURCE_ITEM_MAX) {
    fail(`${resource.path} is ${textBytes} bytes; maximum is ${RESOURCE_ITEM_MAX}`);
  }
  resourceBytes += Buffer.byteLength(resourcePath) + textBytes;
  return { path: resourcePath, text };
});

if (resourceBytes > RESOURCE_TOTAL_MAX) {
  fail(`resource paths and text use ${resourceBytes} bytes; maximum is ${RESOURCE_TOTAL_MAX}`);
}

function push16(out, value) {
  out.push(value & 0xff, (value >>> 8) & 0xff);
}

function push32(out, value) {
  push16(out, value & 0xffff);
  push16(out, value >>> 16);
}

function crc32(bytes) {
  let crc = 0xffff_ffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) {
      crc = (crc >>> 1) ^ ((crc & 1) ? 0xedb8_8320 : 0);
    }
  }
  return (crc ^ 0xffff_ffff) >>> 0;
}

function storeZip(entries) {
  if (entries.length === 0 || entries.length > ENTRY_COUNT_MAX) {
    fail(`ZIP entry count is ${entries.length}; expected 1..${ENTRY_COUNT_MAX}`);
  }
  const output = [];
  const central = [];
  const names = new Set();
  const offsets = [];

  for (const entry of entries) {
    const name = Buffer.from(entry.name, "utf8");
    const data = Buffer.from(entry.text, "utf8");
    if (name.length === 0 || name.length > 0xffff) fail(`ZIP entry name is too long: ${entry.name}`);
    if (data.length > 0xffff_ffff) fail(`ZIP entry is too large: ${entry.name}`);
    if (names.has(entry.name)) fail(`duplicate ZIP entry: ${entry.name}`);
    names.add(entry.name);
    if (output.length > 0xffff_ffff) fail("ZIP local data is too large");
    const offset = output.length;
    const size = data.length;
    const crc = crc32(data);
    offsets.push(offset);

    // Local file header: version 2.0, UTF-8 flag, method 0 (STORE), no
    // optional fields and no data descriptor.
    push32(output, 0x0403_4b50);
    push16(output, 20);
    push16(output, 0x0800);
    push16(output, 0);
    push16(output, 0);
    push16(output, 0);
    push32(output, crc);
    push32(output, size);
    push32(output, size);
    push16(output, name.length);
    push16(output, 0);
    output.push(...name, ...data);
  }

  for (const [index, entry] of entries.entries()) {
    const name = Buffer.from(entry.name, "utf8");
    const data = Buffer.from(entry.text, "utf8");
    const size = data.length;
    const crc = crc32(data);
    push32(central, 0x0201_4b50);
    push16(central, 20);
    push16(central, 20);
    push16(central, 0x0800);
    push16(central, 0);
    push16(central, 0);
    push16(central, 0);
    push32(central, crc);
    push32(central, size);
    push32(central, size);
    push16(central, name.length);
    push16(central, 0);
    push16(central, 0);
    push16(central, 0);
    push16(central, 0);
    push32(central, 0);
    push32(central, offsets[index]);
    central.push(...name);
  }

  const centralOffset = output.length;
  output.push(...central);
  push32(output, 0x0605_4b50);
  push16(output, 0);
  push16(output, 0);
  push16(output, entries.length);
  push16(output, entries.length);
  push32(output, central.length);
  push32(output, centralOffset);
  push16(output, 0);
  if (output.length > PACKAGE_MAX) {
    fail(`.pnw package is ${output.length} bytes; maximum is ${PACKAGE_MAX}`);
  }
  return Buffer.from(output);
}

const entries = [
  { name: "manifest.json", text: manifestBytes.toString("utf8") },
  { name: "main.rn", text: source },
  ...resources.map(resource => ({ name: `resources/${resource.path}`, text: resource.text })),
];
const packageBytes = storeZip(entries);
process.stdout.write(packageBytes);
