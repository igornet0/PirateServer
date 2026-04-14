-- Opaque subscription token for public Xray JSON URL (not derived from session_token hash).
ALTER TABLE grpc_proxy_session ADD COLUMN subscription_token TEXT;

CREATE UNIQUE INDEX grpc_proxy_session_subscription_token_key
  ON grpc_proxy_session (subscription_token)
  WHERE subscription_token IS NOT NULL;
