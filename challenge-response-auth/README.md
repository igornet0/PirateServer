# Challenge–response HMAC (демо-проект)

Автономный вложенный workspace: `challenge-response-auth/` со структурой **`shared/`**, **`server/`**, **`client/`**.

Сборка:

```bash
cargo build --manifest-path challenge-response-auth/Cargo.toml --release
cargo test --manifest-path challenge-response-auth/Cargo.toml
```

## Архитектура

Цель — **общий секрет** + **случайный nonce (≥128 бит)** + **временная метка** + **`session_id`**, проверка **HMAC-SHA256** без самодельной криптографии и с **защитой от replay**.

**Канальный слой.** Сервер поднимает HTTP (Axum, async Tokio):

- `GET /health` — проверка процесса
- `POST /v1/challenge` — выдаёт challenge (**ЭТАП 2**)
- `POST /v1/authenticate` — принимает proof (**ЭТАП 3–4**)

**Общее ядро (`chal-auth-shared`).**

- Генерация nonce: **`rand` + OsRng**, 16 байт (128 бит).
- Канонический ввод под HMAC (чтобы исключить неоднозначность склейки строк): версионированное поле домена **`HCR1-v1\0`**, затем длины + поля `(nonce ‖ timestamp ‖ session_id)` в бинарном виде (`shared/src/crypto.rs`). Это семантический эквивалент вашего шага «склеить участники», но в production-стиле с явными длинами полей.
- Подпись: **`hmac`** + **`sha2`** (`HMAC-SHA256`).
- Сравнение MAC: **`subtle::ConstantTimeEq`** на двух 32-байтных тегах.
- JSON: типы `ChallengeJson`, `AuthAttemptJson` — nonce в Base64, подпись в **нижнем hex** (совместимо с типичным выводом `hex::encode`).

**Защита от replay.**

- Сервер держит in-memory **`HashMap<Uuid, PendingChallenge>`**: для каждой выданной сессии хранится **nonce**, **timestamp challenge** и время создания.
- После **успешной** верификации HMAC строка удаляется; повтор той же связки **`session_id` + proof`** даёт ошибку («unknown or redeemed session»).
- Неудачная попытка (неверный MAC) **не сжигает** сессию до истечения TTL — см. ограничения ниже.

**Expiration.**

- `CHAL_AUTH_TTL_MS`: не использованная сессия протухает реализационно через отказ в `/v1/authenticate` если с момента выдачи challenge прошло слишком много времени по серверным часам.
- `CHAL_AUTH_CLOCK_SKEW_MS`: проверка того, что `timestamp` из challenge не сильно расходится с «сейчас» на сервере (клиент обязан **эхоировать** те же nonce/timestamp/session_id).

## Конфигурация сервера (env)

| Переменная | Назначение |
|------------|------------|
| `CHAL_AUTH_SECRET_HEX` | Секрет, hex-encoded байты (**рекомендуется ≥32 байта**) |
| `CHAL_AUTH_BIND` | Адрес прослушивания (`127.0.0.1:9393` по умолчанию) |
| `CHAL_AUTH_TTL_MS` | TTL необработанного challenge (`chal_auth_shared::CHALLENGE_TTL_MS`) |
| `CHAL_AUTH_CLOCK_SKEW_MS` | допустимый дрейф часов клиента/сети |

## Запуск примеров

**Терминал 1 — сервер:**

```bash
export CHAL_AUTH_SECRET_HEX=$(openssl rand -hex 32)
cargo run --manifest-path challenge-response-auth/Cargo.toml -p chal-auth-server
```

**Терминал 2 — успешная аутентификация** (`--secret-hex` должен **совпасть побайтово** с `CHAL_AUTH_SECRET_HEX`; в новом сеансе shell переменная не подтянется — скопируйте hex строкой или экспортируйте её в этом терминале):

```bash
cargo run --manifest-path challenge-response-auth/Cargo.toml -p chal-auth-client -- \
  --secret-hex "<тот же hex, что был в CHAL_AUTH_SECRET_HEX сервера>" \
  authenticate-ok
```

**Ожидаемый отказ — подделанный MAC:**

```bash
cargo run --manifest-path challenge-response-auth/Cargo.toml -p chal-auth-client -- \
  --secret-hex "<тот же hex>" \
  authenticate-tampered-mac
```

**Ожидаемый отказ — replay второго того же успешного proof:**

```bash
cargo run --manifest-path challenge-response-auth/Cargo.toml -p chal-auth-client -- \
  --secret-hex "<тот же hex>" \
  replay-same-proof-twice
```

> Примечание: в режиме `authenticate-tampered-mac` клиент **намеренно инвертирует бит тега** после расчёта HMAC только как демонстрация ошибки верификации; это не «секретная схема», а мутация готового аутентификатора.

## Рекомендации по эксплуатации

### Хранение shared secret

- **Не коммить** ключи и не передавать их в Slack/email.
- Предпочтительно: **secret manager** (Vault, KMS, systemd credentials, sealed secrets в k8s) или файл с правами **`0600`** + отдельный пользователь процесса.
- Доставка на узел — через **provisioner** или **immutable image** секрет-слоя, избегая shell history (`export SECRET=` оставляет след).

### Ротация ключей

1. Завести **два активных секрета** на переходном окне («старый» + «новый»); сервер принимает MAC по любому из разрешённых, клиент всегда подписывает **новым**.
2. После обновления всех клиентов — отключить старый ключ.
3. Альтернатива без dual-verify: покрашенный rollout + короткий downtime maintenance window (хуже для HA).

У этого демо **нет dual-key** логики; для продакшена нужен явный `active_keys: Vec<[u8]>` или идентификатор ключа (`kid`) в JSON протокола.

### Путь на Ed25519 или mutual TLS

- **Ed25519 (асимметрия):** сервер знает только **открытые** ключи клиентов (allowlist); клиент подписывает тот же canonical payload **`ed25519_dalek` / AWS SigV4-style libs** — убирает долгоживущий shared secret из памяти одного узла как «сломал = доступ всем».

- **mTLS:** криптография и верификация на TLS-стеке (**rustls**/OpenSSL); удобно в интранете; издержки сертификатов и CRL/OCSP/ротации PKI выше операционной сложности HMAC для peer-to-peer.

## Ограничения этого демо

- Против **DoS brute-force MAC** нужны rate-limit (Tower), IP allowlist или WAF перед Axum.

- После ошибки MAC нападающий сохраняет сессию в карте до TTL — можно улучшить политику (ограниченное число попыток / удаление после N промахов с audit log).

## Зависимости (из задания)

- `hmac`, `sha2`, `rand`, `subtle`, `serde`, `tokio`; сеть через **Axum + reqwest**.
