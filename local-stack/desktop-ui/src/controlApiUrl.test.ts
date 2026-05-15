import { describe, expect, it } from "vitest";
import { suggestControlApiFromGrpcUrl } from "./controlApiUrl";

describe("suggestControlApiFromGrpcUrl", () => {
  it("maps http gRPC :50051 to control-api :8080", () => {
    expect(suggestControlApiFromGrpcUrl("http://host:50051")).toBe("http://host:8080");
    expect(suggestControlApiFromGrpcUrl("http://192.168.1.1:50051")).toBe("http://192.168.1.1:8080");
  });

  it("maps https gRPC :50051 to nginx host without :8080", () => {
    expect(suggestControlApiFromGrpcUrl("https://shop.example:50051")).toBe("https://shop.example");
    expect(suggestControlApiFromGrpcUrl("https://shop.tgcryptomarket.ru:50051")).toBe(
      "https://shop.tgcryptomarket.ru",
    );
  });

  it("returns null for empty input", () => {
    expect(suggestControlApiFromGrpcUrl("")).toBeNull();
    expect(suggestControlApiFromGrpcUrl("   ")).toBeNull();
  });
});
