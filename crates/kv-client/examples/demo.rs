use bytes::Bytes;
use kv_client::Client;
use kv_types::chunk_keys;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut client = Client::connect("127.0.0.1:7070").await?;

    let original_tokens: Vec<u32> = (0..64).collect();
    let original_keys = chunk_keys(b"demo-model-v1", &original_tokens, 16)?;
    client
        .put(
            original_keys[..2]
                .iter()
                .map(|key| (*key, Bytes::from(vec![42; 1024])))
                .collect(),
        )
        .await?;

    let mut shared_tokens = original_tokens;
    for token in &mut shared_tokens[32..] {
        *token += 1_000;
    }
    let shared_keys = chunk_keys(b"demo-model-v1", &shared_tokens, 16)?;

    println!(
        "Original reusable chunks: {}",
        client.lookup(original_keys).await?
    );
    println!(
        "Shared-prefix reusable chunks: {}",
        client.lookup(shared_keys.clone()).await?
    );
    let chunks = client.get(shared_keys[..3].to_vec()).await?;
    let sizes: Vec<_> = chunks
        .iter()
        .map(|value| value.as_ref().map(Bytes::len))
        .collect();
    println!("Retrieved chunk sizes: {sizes:?}");
    println!("Cache payload bytes: {}", client.used_bytes().await?);
    Ok(())
}
