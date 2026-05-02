use std::path::PathBuf;
use std::time::Instant;

use ashta_core::{Event, EventKind, SymbolId};
use ashta_log::LogWriter;
use serde::Deserialize;
use tungstenite::{connect, Message};

/// URL du flux trade Binance — BTC/USDT, tick par tick.
const URL: &str = "wss://stream.binance.com/ws/btcusdt@trade";

/// Répertoire de log persistant sur la VM.
const LOG_DIR: &str = "/tmp/ashta_ws_log";

/// Fréquence d'affichage : 1 ligne toutes les N trades.
const PRINT_EVERY: usize = 50;

/// Sous-ensemble du message Binance `@trade` utile pour Ashta.
///
/// Binance envoie un objet JSON — seuls les champs mappés ici
/// sont désérialisés ; les autres sont ignorés par serde.
///
/// Référence : https://binance-docs.github.io/apidocs/spot/en/#trade-streams
#[derive(Deserialize)]
struct TradeMsg {
    /// Temps d'exécution du trade — millisecondes depuis l'epoch Unix.
    #[serde(rename = "T")]
    trade_time_ms: u64,

    /// Prix exécuté, encodé en chaîne décimale par Binance.
    /// Ex : `"67432.10"`
    #[serde(rename = "p")]
    price: String,

    /// Volume exécuté (quantité BTC), encodé en chaîne décimale.
    /// Ex : `"0.00123"`
    #[serde(rename = "q")]
    quantity: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ── 1. Ouvre (ou reprend) le log ──────────────────────────────────────
    let dir = PathBuf::from(LOG_DIR);
    std::fs::create_dir_all(&dir)?;
    let mut writer = LogWriter::open(&dir)?;

    let symbol = SymbolId::from("BTCUSDT");

    // ── 2. Connexion WebSocket (TLS via rustls) ───────────────────────────
    println!("Connecting to {URL} ...");
    let (mut ws, _response) = connect(URL)?;
    println!("Connected.  Log → {LOG_DIR}\n");

    println!("{:<8}  {:>14}  {:>12}  {:>10}", "trade#", "price (USD)", "volume (BTC)", "events/s");
    println!("{}", "─".repeat(54));

    let mut count = 0usize;
    let t_start = Instant::now();

    // ── 3. Boucle de lecture ──────────────────────────────────────────────
    loop {
        let msg = ws.read()?;

        let text = match msg {
            Message::Text(t) => t,
            // Répondre aux Ping pour maintenir la connexion ouverte.
            Message::Ping(data) => {
                ws.send(Message::Pong(data))?;
                continue;
            }
            Message::Close(_) => {
                println!("\nServeur a fermé la connexion.");
                break;
            }
            // Binary / Pong / Frame → ignoré
            _ => continue,
        };

        // Certains messages du flux ne sont pas des trades (ex : ping applicatif).
        let trade: TradeMsg = match serde_json::from_str(&text) {
            Ok(t) => t,
            Err(_) => continue,
        };

        // ── 4. Conversion JSON → Event ────────────────────────────────────
        let event = Event {
            // Binance donne des ms, Ashta stocke des ns.
            timestamp_ns: trade.trade_time_ms * 1_000_000,
            symbol,
            price: trade.price.parse::<f64>().unwrap_or(0.0),
            volume: trade.quantity.parse::<f64>().unwrap_or(0.0),
            kind: EventKind::Trade,
            _pad: [0; 7],
        };

        // ── 5. Écriture dans le log ───────────────────────────────────────
        writer.append(&event)?;
        count += 1;

        // ── 6. Affichage périodique ───────────────────────────────────────
        if count % PRINT_EVERY == 0 {
            let elapsed = t_start.elapsed();
            let rate = count as f64 / elapsed.as_secs_f64();
            println!(
                "{:<8}  {:>14.2}  {:>12.5}  {:>10.1}",
                count, event.price, event.volume, rate,
            );
        }
    }

    println!("\nTotal : {count} events en {:.2?}", t_start.elapsed());
    Ok(())
}
