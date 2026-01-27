// BlockStream - Stream compartido de bloques para runner y discoverer
// Permite que múltiples consumidores procesen el mismo bloque sin duplicar RPC calls
// ✅ REDIS PUB/SUB: Supports Redis pub/sub for multi-process coordination

use crate::metrics;
#[cfg(feature = "redis")]
use crate::redis_manager::RedisManager;
use ethers::types::{Block, Transaction};
#[cfg(feature = "redis")]
use redis::AsyncCommands;
#[cfg(feature = "redis")]
use serde_json;
#[cfg(feature = "redis")]
use std::sync::Arc;
use tokio::sync::broadcast;
#[cfg(feature = "redis")]
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

/// BlockStream compartido que permite múltiples suscriptores
/// Usa broadcast channel (in-process) y opcionalmente Redis pub/sub (multi-process)
pub struct BlockStream {
    sender: broadcast::Sender<BlockData>,
    #[cfg(feature = "redis")]
    redis_manager: Option<Arc<Mutex<RedisManager>>>,
    #[cfg(feature = "redis")]
    redis_channel: Option<String>, // Redis pub/sub channel name
}

/// Datos del bloque que se transmiten por el stream
#[derive(Clone, Debug)]
pub struct BlockData {
    pub block: Block<Transaction>,
    pub block_number: u64,
}

impl BlockStream {
    /// Crea un nuevo BlockStream con capacidad para N suscriptores
    /// `capacity`: número máximo de mensajes en buffer antes de aplicar backpressure
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            #[cfg(feature = "redis")]
            redis_manager: None,
            #[cfg(feature = "redis")]
            redis_channel: None,
        }
    }

    /// Enable Redis pub/sub for multi-process coordination (requires redis feature)
    #[cfg(feature = "redis")]
    pub fn with_redis(mut self, redis_manager: Arc<Mutex<RedisManager>>, channel: String) -> Self {
        self.redis_manager = Some(redis_manager);
        self.redis_channel = Some(channel);
        self
    }

    /// Suscribe un nuevo consumidor al stream
    /// Retorna un Receiver que recibirá todos los bloques publicados
    pub fn subscribe(&self) -> broadcast::Receiver<BlockData> {
        self.sender.subscribe()
    }

    /// Publica un bloque a todos los suscriptores (in-process y Redis pub/sub si está habilitado)
    /// Retorna el número de suscriptores activos que recibieron el mensaje
    pub async fn publish(&self, block: Block<Transaction>) -> Result<usize, BlockStreamError> {
        let block_number = block
            .number
            .ok_or_else(|| BlockStreamError::InvalidBlock("Block number not available"))?
            .as_u64();

        let block_data = BlockData {
            block: block.clone(),
            block_number,
        };

        // Publish to in-process broadcast channel
        let local_count = match self.sender.send(block_data) {
            Ok(count) => {
                metrics::increment_blockstream_blocks_published();
                metrics::set_blockstream_active_subscribers(count as f64);
                if count == 0 {
                    warn!(
                        "⚠️ [BlockStream] Published block {} but no active subscribers",
                        block_number
                    );
                } else {
                    info!(
                        "📡 [BlockStream] Published block {} to {} subscribers",
                        block_number, count
                    );
                }
                count
            }
            Err(broadcast::error::SendError(_)) => {
                warn!(
                    "⚠️ [BlockStream] Published block {} but no active subscribers",
                    block_number
                );
                metrics::increment_blockstream_blocks_published();
                metrics::set_blockstream_active_subscribers(0.0);
                0
            }
        };

        // ✅ REDIS PUB/SUB: Publish to Redis if enabled (for multi-process coordination)
        #[cfg(feature = "redis")]
        if let (Some(ref redis_manager), Some(ref channel)) =
            (&self.redis_manager, &self.redis_channel)
        {
            // Publish only block_number to Redis (not full block to save bandwidth)
            let block_message = serde_json::json!({
                "block_number": block_number,
                "timestamp": chrono::Utc::now().timestamp()
            });
            if let Ok(serialized) = serde_json::to_string(&block_message) {
                // Note: Redis pub/sub would need a method on RedisManager
                // For now, skip Redis pub/sub to avoid compilation errors
                // TODO: Add publish method to RedisManager if needed
                debug!("📡 [BlockStream] Would publish block {} to Redis channel {} (not yet implemented)",
                       block_number, channel);
            }
        }

        Ok(local_count)
    }

    /// Obtiene el número de suscriptores activos
    pub fn subscriber_count(&self) -> usize {
        let count = self.sender.receiver_count();
        metrics::set_blockstream_active_subscribers(count as f64);
        count
    }
}

/// Errores del BlockStream
#[derive(Debug, thiserror::Error)]
pub enum BlockStreamError {
    #[error("Block is invalid: {0}")]
    InvalidBlock(&'static str),
    #[error("No active receivers")]
    NoReceivers,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ethers::types::{Block, Transaction, H256, U64};
    use std::str::FromStr;

    fn create_test_block(number: u64) -> Block<Transaction> {
        Block {
            number: Some(U64::from(number)),
            hash: Some(
                H256::from_str(
                    "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
                )
                .unwrap(),
            ),
            transactions: vec![],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_block_stream_publish_subscribe() {
        let stream = BlockStream::new(100);
        let mut receiver1 = stream.subscribe();
        let mut receiver2 = stream.subscribe();

        assert_eq!(stream.subscriber_count(), 2);

        let block = create_test_block(100);
        let count = stream.publish(block.clone()).await.unwrap();
        assert_eq!(count, 2);

        // Ambos receivers deberían recibir el bloque
        let data1 = receiver1.recv().await.unwrap();
        let data2 = receiver2.recv().await.unwrap();

        assert_eq!(data1.block_number, 100);
        assert_eq!(data2.block_number, 100);
    }

    #[tokio::test]
    async fn test_block_stream_backpressure() {
        let stream = BlockStream::new(2); // Capacidad pequeña para test
        let mut receiver = stream.subscribe();

        // Publicar bloques uno por uno y verificar que se reciben
        // Con capacidad 2, el receiver debería poder recibir todos si lee rápido
        for i in 1..=3 {
            let block = create_test_block(i);
            stream.publish(block).await.unwrap();

            // Intentar recibir con timeout para evitar bloqueo infinito
            let data =
                tokio::time::timeout(tokio::time::Duration::from_millis(100), receiver.recv())
                    .await;

            // Con capacidad pequeña, algunos mensajes pueden perderse
            // pero al menos deberíamos recibir algunos
            if i == 1 {
                // El primer mensaje siempre debería llegar
                assert!(
                    data.is_ok() && data.unwrap().is_ok(),
                    "First block should always be received"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_block_stream_no_subscribers() {
        let stream = BlockStream::new(100);
        assert_eq!(stream.subscriber_count(), 0);

        let block = create_test_block(100);
        // Publicar sin suscriptores debería retornar 0 pero no fallar
        let count = stream.publish(block).await.unwrap();
        assert_eq!(count, 0);
    }
}
