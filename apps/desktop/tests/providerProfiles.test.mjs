import assert from "node:assert/strict";
import test from "node:test";
import { providerProfilesMatch } from "../src/providerProfiles.ts";

const saved = {
  baseUrl: "https://api.example.com/v1/",
  providerName: "Example API",
  model: "gpt-5.6",
  apiKey: "sk-saved",
};

test("remembered provider still matches when the live key is unavailable", () => {
  assert.equal(providerProfilesMatch(saved, {
    baseUrl: "https://API.EXAMPLE.com/v1",
    providerName: " example   api ",
    model: "gpt-5.6",
    apiKey: "",
  }), true);
});

test("different models or two different explicit keys do not match", () => {
  assert.equal(providerProfilesMatch(saved, { ...saved, model: "deepseek-v3" }), false);
  assert.equal(providerProfilesMatch(saved, { ...saved, apiKey: "sk-other" }), false);
});

test("matching explicit credentials identify the same profile even after a rename", () => {
  assert.equal(providerProfilesMatch(saved, {
    ...saved,
    providerName: "Renamed copy",
  }), true);
});
