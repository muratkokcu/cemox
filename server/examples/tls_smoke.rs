//! Gerçek bir HTTPS ucuna istek atarak rustls yapılandırmasını doğrular.
//! Teste değil örneğe konuldu: ağ bağlantısı gerektiriyor.
#[tokio::main]
async fn main() {
    let client = reqwest::Client::builder()
        .build()
        .expect("istemci kurulamadı");
    match client
        .get("https://oauth2.googleapis.com/token")
        .send()
        .await
    {
        Ok(response) => println!("TLS çalışıyor — Google yanıtı: {}", response.status()),
        Err(error) => {
            eprintln!("TLS başarısız: {error}");
            std::process::exit(1);
        }
    }
}
