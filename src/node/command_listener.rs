use std::sync::Arc;

use anyhow::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::Mutex,
};

use crate::{
    models::{
        app_context::{AppContext, Response},
        db::InMemoryDb,
    },
    node::command_handlers,
    resp_parser::{self, shared::RespCommandNames},
    TCP_READ_TIMEOUT, TCP_READ_TIMEOUT_MAX_RETRIES,
};

pub(crate) async fn run(mem_db: &Arc<Mutex<InMemoryDb>>) -> Result<(), Error> {
    let listening_port = {
        let db_lock = mem_db.lock().await;
        db_lock.get_app_data_ref().listening_port
    };

    let listener = TcpListener::bind(format!("127.0.0.1:{}", listening_port)).await?;

    loop {
        let _ = match listener.accept().await {
            Ok((mut _tcp_stream, addy)) => {
                println!("accepted new connection from {}", addy.ip());

                let mem_db_arc_pointer = Arc::clone(&mem_db);

                tokio::spawn(async move {
                    match handle_client_connection(&mem_db_arc_pointer, &mut _tcp_stream).await {
                        Err(e) => {
                            println!("connection handling error: {}", e);
                        }
                        Ok(()) => (),
                    }

                    println!("finished handling request");
                });
            }
            Err(e) => {
                println!("tcp connection error: {}", e);
            }
        };
    }
}

async fn handle_client_connection<'a>(
    mem_db: &Arc<Mutex<InMemoryDb>>,
    tcp_stream: &mut TcpStream,
) -> Result<(), anyhow::Error> {
    let mut num_of_retries = 0;

    loop {
        let mut request_buffer: [u8; 1024] = [0; 1024];

        println!("reading request");

        match tokio::time::timeout(TCP_READ_TIMEOUT, tcp_stream.read(&mut request_buffer)).await {
            Err(e) => {
                println!("timeout while reading request - {}", e);

                if num_of_retries == TCP_READ_TIMEOUT_MAX_RETRIES {
                    break;
                }

                num_of_retries += 1;
                continue;
            }
            Ok(read_result) => {
                let request_byte_count = read_result?;

                println!("request received of len {}", request_byte_count);

                if request_byte_count == 0 {
                    // The socket is closed.
                    break;
                }

                let command_response =
                    // `request_byte_count + 1` to pad it with a 0.
                    handle_command(mem_db, &request_buffer[..request_byte_count + 1]).await?;

                println!("sending response - {:?}", command_response);
                tcp_stream.write_all(command_response.as_bytes()).await?;
                tcp_stream.flush().await?;
            }
        };

        println!("finished reading request");
    }

    Ok(())
}

async fn handle_command<'a>(
    mem_db: &Arc<Mutex<InMemoryDb>>,
    request_buffer: &[u8],
) -> Result<String, anyhow::Error> {
    let mut context = AppContext::new(mem_db, &request_buffer)?;

    println!("parsing request");

    resp_parser::parse_resp_proc_command(&mut context)?;

    println!("handling request: {}", context.format_request_info(true)?);

    match context
        .get_request_resp_command_ref()
        .unwrap()
        .name
        .as_str()
    {
        RespCommandNames::PING => command_handlers::handle_command_ping(&mut context)?,
        RespCommandNames::REPLCONF => command_handlers::handle_command_replconf(&mut context)?,
        RespCommandNames::PSYNC => command_handlers::handle_command_psync(&mut context).await?,
        RespCommandNames::ECHO => command_handlers::handle_command_echo(&mut context)?,
        RespCommandNames::GET => command_handlers::handle_command_get_async(&mut context).await?,
        RespCommandNames::SET => command_handlers::handle_command_set_async(&mut context).await?,
        RespCommandNames::INFO => command_handlers::handle_command_info(&mut context).await?,

        _ => {
            return Err(Error::msg(
                "Could not handle command - Unknown or not implemented command.",
            ))
        }
    };

    Ok(context.unwrap_response_command_response().to_owned())
}

#[cfg(test)]
mod tests {
    use crate::{
        models::app_context::AppContext,
        node::command_handlers::{
            handle_command_echo, handle_command_get_async, handle_command_ping,
            handle_command_set_async,
        },
        node::command_listener::handle_command,
        resp_parser::{parse_resp_proc_command, shared::RespCommandNames},
        test_helpers::utils::create_test_mem_db,
    };

    use anyhow::Ok;

    #[tokio::test]
    async fn handle_command_handles_ping() -> Result<(), anyhow::Error> {
        let request_buffer = b"*1\r\n$4\r\npiNg\r\n";
        let fake_mem_db = create_test_mem_db()?;
        let mut fake_app_context = AppContext::new(&fake_mem_db, request_buffer)?;

        parse_resp_proc_command(&mut fake_app_context)?;
        assert_eq!(
            fake_app_context
                .get_request_resp_command_ref()
                .unwrap()
                .name,
            RespCommandNames::PING
        );
        assert_eq!(
            fake_app_context
                .get_request_resp_command_ref()
                .unwrap()
                .parameters
                .len(),
            0
        );

        handle_command_ping(&mut fake_app_context)?;
        let handled_command_response = handle_command(&fake_mem_db, request_buffer).await?;
        assert_eq!(
            fake_app_context
                .unwrap_response_command_response()
                .to_owned(),
            "+PONG\r\n".to_owned()
        );
        assert_eq!(handled_command_response, "+PONG\r\n".to_owned());

        Ok(())
    }

    #[tokio::test]
    async fn handle_command_handles_echo() -> Result<(), anyhow::Error> {
        let request_buffer = b"*2\r\n$4\r\nEcHo\r\n$19\r\nHey world, I'm Joe!\r\n";
        let fake_mem_db = create_test_mem_db()?;
        let mut fake_app_context = AppContext::new(&fake_mem_db, request_buffer)?;

        parse_resp_proc_command(&mut fake_app_context)?;
        assert_eq!(
            fake_app_context
                .get_request_resp_command_ref()
                .unwrap()
                .name,
            RespCommandNames::ECHO
        );
        assert_eq!(
            fake_app_context
                .get_request_resp_command_ref()
                .unwrap()
                .parameters
                .len(),
            1
        );

        handle_command_echo(&mut fake_app_context)?;
        let handled_command_response = handle_command(&fake_mem_db, request_buffer).await?;
        assert_eq!(
            fake_app_context
                .unwrap_response_command_response()
                .to_owned(),
            "$19\r\nHey world, I'm Joe!\r\n".to_owned()
        );
        assert_eq!(
            handled_command_response,
            "$19\r\nHey world, I'm Joe!\r\n".to_owned()
        );

        Ok(())
    }

    #[tokio::test]
    async fn handle_command_handles_set_get() -> Result<(), anyhow::Error> {
        let fake_mem_db = create_test_mem_db()?;

        // Set:
        let request_buffer_set = b"*3\r\n$3\r\nsET\r\n$3\r\nfoo\r\n$19\r\nHey world, I'm Joe!\r\n";
        // let request_buffer_set = b"*3\r\n$3\r\nsET\r\n$3\r\nfoo\r\n$19\r\nHey world, I'm Joe!\r\n$2\r\nPx\r\n$3\r\n100\r\n";
        let mut fake_app_context_set = AppContext::new(&fake_mem_db, request_buffer_set)?;

        parse_resp_proc_command(&mut fake_app_context_set)?;
        assert_eq!(
            fake_app_context_set
                .get_request_resp_command_ref()
                .unwrap()
                .name,
            RespCommandNames::SET
        );
        assert_eq!(
            fake_app_context_set
                .get_request_resp_command_ref()
                .unwrap()
                .parameters
                .len(),
            2
        );

        handle_command_set_async(&mut fake_app_context_set).await?;
        let handled_command_response_set = handle_command(&fake_mem_db, request_buffer_set).await?;
        assert_eq!(
            fake_app_context_set
                .unwrap_response_command_response()
                .to_owned(),
            "+OK\r\n".to_owned()
        );
        assert_eq!(handled_command_response_set, "+OK\r\n".to_owned());

        // Get:
        let request_buffer_get = b"*2\r\n$3\r\ngET\r\n$3\r\nfoo\r\n";
        let mut fake_app_context_get = AppContext::new(&fake_mem_db, request_buffer_get)?;

        parse_resp_proc_command(&mut fake_app_context_get)?;
        assert_eq!(
            fake_app_context_get
                .get_request_resp_command_ref()
                .unwrap()
                .name,
            RespCommandNames::GET
        );
        assert_eq!(
            fake_app_context_get
                .get_request_resp_command_ref()
                .unwrap()
                .parameters
                .len(),
            1
        );

        handle_command_get_async(&mut fake_app_context_get).await?;
        let handled_command_response_get = handle_command(&fake_mem_db, request_buffer_get).await?;
        assert_eq!(
            fake_app_context_get
                .unwrap_response_command_response()
                .to_owned(),
            "$19\r\nHey world, I'm Joe!\r\n".to_owned()
        );
        assert_eq!(
            handled_command_response_get,
            "$19\r\nHey world, I'm Joe!\r\n".to_owned()
        );

        Ok(())
    }

    // #[tokio::test]
    // async fn handle_command_handles_info() -> Result<(), anyhow::Error> {
    //     todo!()
    // }
}
