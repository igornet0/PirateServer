import { describe, expect, it } from "vitest";
import { looksLikeInstallBundle } from "./installBundle";

describe("looksLikeInstallBundle", () => {
  it("accepts full install JSON", () => {
    expect(
      looksLikeInstallBundle(
        '{"token":"t","url":"http://x:50051","pairing":"abc"}',
      ),
    ).toBe(true);
  });

  it("rejects url-only JSON", () => {
    expect(looksLikeInstallBundle('{"url":"http://x:50051"}')).toBe(false);
  });

  it("rejects plain gRPC URL", () => {
    expect(looksLikeInstallBundle("http://x:50051")).toBe(false);
  });
});
