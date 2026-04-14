-- Inbound wire protocol metadata (VLESS / Trojan / VMess) for managed proxy sessions.

ALTER TABLE grpc_proxy_session ADD COLUMN wire_mode SMALLINT NULL;
ALTER TABLE grpc_proxy_session ADD COLUMN wire_config_json TEXT NULL;
