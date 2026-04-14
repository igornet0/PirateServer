-- Full ingress (sing-box compatible metadata): public listeners separate from gRPC ProxyWireMode.

ALTER TABLE grpc_proxy_session ADD COLUMN ingress_protocol SMALLINT NULL;
ALTER TABLE grpc_proxy_session ADD COLUMN ingress_listen_port INTEGER NULL;
ALTER TABLE grpc_proxy_session ADD COLUMN ingress_listen_udp_port INTEGER NULL;
ALTER TABLE grpc_proxy_session ADD COLUMN ingress_config_json TEXT NULL;
ALTER TABLE grpc_proxy_session ADD COLUMN ingress_tls_json TEXT NULL;
ALTER TABLE grpc_proxy_session ADD COLUMN ingress_template_version INTEGER NOT NULL DEFAULT 1;
