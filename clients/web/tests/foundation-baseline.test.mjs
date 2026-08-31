import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";

import { assertAdministratorWebToolchain } from "@sarmg/admin-web";

const directory = new URL("../", import.meta.url);
const packageJson = JSON.parse(await readFile(new URL("package.json", directory), "utf8"));
const lock = JSON.parse(await readFile(new URL("package-lock.json", directory), "utf8"));
const nodeVersion = await readFile(new URL("../../.node-version", directory), "utf8");
const source = await readFile(new URL("src/main.tsx", directory), "utf8");
const html = await readFile(new URL("index.html", directory), "utf8");
const installedDesignPackage = JSON.parse(
  await readFile(new URL("node_modules/@sarmg/design-tokens/package.json", directory), "utf8"),
);
const foundationReleaseBase =
  "https://github.com/isarmg/sarmg-foundation/releases/download/v0.3.0";
const foundationPackages = ["admin-web", "contracts", "design-tokens", "http-client"];

assertAdministratorWebToolchain(packageJson, nodeVersion);
for (const name of [
  "react", "react-dom", "@types/react", "@types/react-dom",
  "vite", "@vitejs/plugin-react", "typescript",
]) {
  const declared = packageJson.dependencies?.[name] ?? packageJson.devDependencies?.[name];
  assert.equal(lock.packages[`node_modules/${name}`]?.version, declared);
}
for (const name of foundationPackages) {
  const dependency = `@sarmg/${name}`;
  const expected = `${foundationReleaseBase}/sarmg-${name}-0.3.0.tgz`;
  assert.equal(packageJson.dependencies[dependency], expected);
  assert.equal(lock.packages[""].dependencies[dependency], expected);

  const locked = lock.packages[`node_modules/${dependency}`];
  assert.equal(locked?.version, "0.3.0");
  assert.equal(locked?.resolved, expected);
  assert.match(locked?.integrity ?? "", /^sha512-[A-Za-z0-9+/]+={0,2}$/);
}
assert.equal(installedDesignPackage.version, "0.3.0");
for (const name of ["tokens.css", "reset.css", "accessibility.css"]) {
  assert.match(source, new RegExp(`import "@sarmg/design-tokens/${name.replace(".", "\\.")}";`));
}
const expectedDigests = {
  "tokens.css": "124b788529faf5031ff7b12ac7c5493a1ceb3d11c76693bfa7e5d971f22547d4",
  "reset.css": "54556e5d22e275fe9aafdaca468056d17e09da3de93729637c0f2481a8f26eab",
  "accessibility.css": "8153af37ecc40a69c1305f8179777e0c60b1f2a730bf3ebfcebe43aecf9df0bb",
};
for (const [name, expected] of Object.entries(expectedDigests)) {
  const content = await readFile(new URL(`node_modules/@sarmg/design-tokens/dist/${name}`, directory));
  assert.equal(createHash("sha256").update(content).digest("hex"), expected);
}
assert.match(source, /useAdministratorSession/);
assert.match(source, /@sarmg\/contracts/);
assert.match(source, /@sarmg\/http-client/);
assert.match(source, /createRoot/);
assert.match(source, /name="username"/);
assert.match(source, /session\.username/);
assert.doesNotMatch(source, /name="email"|session\.email|管理员邮箱/);
assert.match(html, /<body\s+data-sarmg-scope>/);
assert.doesNotMatch(source, /vendor\/sarmg-design/);
assert.doesNotMatch(source, /["'`]\/v2\/admin/);

console.log("Media Backup Foundation Web 基线验证通过");
