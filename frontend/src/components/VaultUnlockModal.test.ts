import { test } from "node:test";
import assert from "node:assert/strict";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { VaultUnlockModal } from "./VaultUnlockModal.tsx";
import type { VaultStatus } from "../types/index.ts";

const noop = () => {};

test("VaultUnlockModal renders Create Master Password when uninitialized without paper mode bypass", () => {
  const uninitializedStatus: VaultStatus = {
    is_initialized: false,
    is_unlocked: false,
  };

  const html = renderToStaticMarkup(
    React.createElement(VaultUnlockModal, {
      isOpen: true,
      vaultStatus: uninitializedStatus,
      onClose: noop,
      onSuccess: noop,
    })
  );

  assert.match(html, /Create Master Password/);
  assert.match(html, /Create Master Password &amp; Enter/);
  assert.match(html, /backdrop-blur-3xl/);
  assert.match(html, /blur\(32px\)/);
  // Ensure paper mode bypass is completely absent
  assert.doesNotMatch(html, /Paper Mode/);
  assert.doesNotMatch(html, /Skip/);
});

test("VaultUnlockModal renders Unlock Workspace when initialized & locked without paper mode bypass", () => {
  const lockedStatus: VaultStatus = {
    is_initialized: true,
    is_unlocked: false,
  };

  const html = renderToStaticMarkup(
    React.createElement(VaultUnlockModal, {
      isOpen: true,
      vaultStatus: lockedStatus,
      onClose: noop,
      onSuccess: noop,
    })
  );

  assert.match(html, /Unlock Workspace/);
  assert.match(html, /backdrop-blur-3xl/);
  assert.match(html, /blur\(32px\)/);
  // Ensure paper mode bypass is completely absent
  assert.doesNotMatch(html, /Paper Mode/);
  assert.doesNotMatch(html, /Skip/);
  assert.doesNotMatch(html, /Continue/);
});

test("VaultUnlockModal change mode collects current + new + confirm", () => {
  const unlockedStatus: VaultStatus = {
    is_initialized: true,
    is_unlocked: true,
  };

  const html = renderToStaticMarkup(
    React.createElement(VaultUnlockModal, {
      isOpen: true,
      vaultStatus: unlockedStatus,
      mode: "change",
      dismissible: true,
      onClose: noop,
      onSuccess: noop,
    })
  );

  assert.match(html, /Change Master Password/);
  assert.match(html, /Current Password/);
  assert.match(html, /New Password/);
  assert.match(html, /Confirm Password/);
  assert.match(html, /EVM wallet/);
});

test("VaultUnlockModal renders nothing when isOpen is false", () => {
  const lockedStatus: VaultStatus = {
    is_initialized: true,
    is_unlocked: false,
  };

  const html = renderToStaticMarkup(
    React.createElement(VaultUnlockModal, {
      isOpen: false,
      vaultStatus: lockedStatus,
      onClose: noop,
      onSuccess: noop,
    })
  );

  assert.equal(html, "");
});
