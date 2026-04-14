ALTER TABLE grpc_proxy_session ADD COLUMN subscription_token TEXT;

CREATE UNIQUE INDEX grpc_proxy_session_subscription_token_key
  ON grpc_proxy_session (subscription_token)
  WHERE subscription_token IS NOT NULL;
