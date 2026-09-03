use crate::AppState;
use serde_json::json;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};
use tauri::{AppHandle, Emitter};
use base64::{engine::general_purpose::STANDARD, Engine as _};

pub struct LocalServer {
    stop: Arc<AtomicBool>,
}

impl LocalServer {
    pub fn start(app: AppHandle, state: Arc<AppState>, port: u16) -> std::io::Result<Self> {
        // LAN control is opt-in at the configuration layer. Once enabled,
        // bind all interfaces so a phone on the same network can connect.
        let listener = TcpListener::bind(("0.0.0.0", port))?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let signal = stop.clone();
        thread::spawn(move || {
            while !signal.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let connection_app = app.clone();
                        let connection_state = state.clone();
                        thread::spawn(move || {
                            let _ = handle(&mut stream, &connection_app, &connection_state);
                        });
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(40))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self { stop })
    }
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

const MOBILE_PAGE_V2: &str = r#"<!doctype html><meta charset=utf-8><meta name=viewport content="width=device-width,initial-scale=1"><title>宝宝巴士控制台</title><style>body{font:18px system-ui,sans-serif;max-width:760px;margin:24px auto;padding:0 16px;background:#10141f;color:#e8edf7}h1{font-size:30px}p{color:#aabbd6}.buttons{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:14px}button{min-height:70px;padding:16px 12px;font-size:20px;font-weight:600;border-radius:14px;border:1px solid #496997;background:#29436d;color:#fff}.card{background:#18243a;border-radius:12px;padding:14px;margin:16px 0}.thumbs{display:flex;gap:8px;flex-wrap:wrap}.thumbs img{width:150px;height:100px;object-fit:cover;border-radius:7px}.answer{white-space:pre-wrap;line-height:1.55;overflow-wrap:anywhere}@media(max-width:520px){.buttons{grid-template-columns:1fr}button{min-height:76px}}</style><h1>宝宝巴士控制台</h1><p>同一局域网内使用。截图后点击提交。</p><div class=buttons><button onclick="cmd('capture','general')">通用截图</button><button onclick="cmd('capture','math')">数学截图</button><button onclick="cmd('capture','code')">代码截图</button><button onclick="cmd('submit')">提交题目</button><button onclick="cmd('clear')">清空草稿</button><button onclick="cmd('cancel')">取消任务</button></div><div id=view class=card>加载中…</div><script>const saved=new Set;async function cmd(c,p){await fetch('/api/'+c+(p?'?preset='+p:''),{method:'POST'});load()}function esc(v){return String(v??'').replace(/[&<>"']/g,m=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[m]))}function saveImages(d){if(!d.config?.mobile_auto_save_images||!d.draft?.thumbnails)return;d.draft.thumbnails.forEach((src,i)=>{const key=d.draft.id+':'+i;if(saved.has(key))return;saved.add(key);const a=document.createElement('a');a.href=src;a.download='baobao-bashi-'+d.draft.id+'-'+(i+1)+'.png';a.click()})}function render(d){saveImages(d);let h='<b>状态：</b>'+esc(d.status||'')+'<br>'+esc(d.message||'');if(d.draft){h+='<h3>完整截图预览（'+d.draft.image_count+' 张）</h3><div class=thumbs>'+(d.draft.thumbnails||[]).map((x,i)=>'<img src="'+x+'" alt="截图 '+(i+1)+'">').join('')+'</div>'}if(d.stream_output)h+='<h3>流式输出</h3><div class=answer>'+esc(d.stream_output)+'</div>';if(d.answer)h+='<h3>答案</h3><div class=answer>'+esc(d.answer.text)+'</div>';view.innerHTML=h}async function load(){try{render(await (await fetch('/api/state')).json())}catch(e){view.textContent='连接失败：'+e}}load();setInterval(load,1000)</script>"#;

const MOBILE_PAGE_V3: &str = r#"<!doctype html><meta charset=utf-8><meta name=viewport content="width=device-width,initial-scale=1"><title>宝宝巴士控制台</title><style>body{font:18px system-ui,sans-serif;max-width:760px;margin:24px auto;padding:0 16px;background:#10141f;color:#e8edf7}h1{font-size:30px}p{color:#aabbd6}.buttons{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:14px}button{min-height:70px;padding:16px 12px;font-size:20px;font-weight:600;border-radius:14px;border:1px solid #496997;background:#29436d;color:#fff;transition:transform .08s,background .08s}button:active,button.busy{transform:scale(.96);background:#416fb8}.card{background:#18243a;border-radius:12px;padding:14px;margin:16px 0}.thumbs{display:flex;gap:8px;flex-wrap:wrap}.thumbs img{width:150px;height:100px;object-fit:cover;border-radius:7px}.answer{white-space:pre-wrap;line-height:1.55;overflow-wrap:anywhere}.conn{min-height:1.4em;color:#f5c26b}@media(max-width:520px){.buttons{grid-template-columns:1fr}button{min-height:76px}}</style><h1>宝宝巴士控制台</h1><p>同一局域网内使用。截图后点击提交。</p><div class=buttons><button data-action="capture" data-preset="general">通用截图</button><button data-action="capture" data-preset="math">数学截图</button><button data-action="capture" data-preset="code">代码截图</button><button data-action="submit">提交题目</button><button data-action="clear">清空草稿</button><button data-action="cancel">取消任务</button></div><div id=conn class=conn></div><div id=state class=card>加载中…</div><script>const saved=new Set;let last=null,retry=1000;const stateEl=document.getElementById('state'),connEl=document.getElementById('conn');function esc(v){return String(v??'').replace(/[&<>"']/g,m=>({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[m]))}function saveImages(d){if(!d.config?.mobile_auto_save_images||!d.draft?.thumbnails)return;d.draft.thumbnails.forEach((src,i)=>{const key=d.draft.id+':'+i;if(saved.has(key))return;saved.add(key);const a=document.createElement('a');a.href=src;a.download='baobao-bashi-'+d.draft.id+'-'+(i+1)+'.png';a.click()})}function render(d){saveImages(d);let h='<b>状态：</b>'+esc(d.status||'')+'<br>'+esc(d.message||'');if(d.draft){h+='<h3>完整截图预览（'+d.draft.image_count+' 张）</h3><div class=thumbs>'+(d.draft.thumbnails||[]).map((x,i)=>'<img src="'+x+'" alt="截图 '+(i+1)+'">').join('')+'</div>'}if(d.stream_output)h+='<h3>流式输出</h3><div class=answer>'+esc(d.stream_output)+'</div>';if(d.answer)h+='<h3>答案</h3><div class=answer>'+esc(d.answer.text)+'</div>';stateEl.innerHTML=h}function setConn(ok,msg){connEl.textContent=ok?'':(msg||'连接暂时中断，正在重试…')}async function load(){const ctl=new AbortController,t=setTimeout(()=>ctl.abort(),4000);try{const r=await fetch('/api/state',{cache:'no-store',signal:ctl.signal});if(!r.ok)throw Error('HTTP '+r.status);last=await r.json();render(last);setConn(true);retry=1000}catch(e){setConn(false);retry=Math.min(Math.round(retry*1.5),10000)}finally{clearTimeout(t);setTimeout(load,retry)}}async function cmd(c,p){const b=[...document.querySelectorAll('button')].find(x=>x.dataset.action===c&&(!p||x.dataset.preset===p));if(b){b.classList.add('busy');b.disabled=true}try{const r=await fetch('/api/'+c+(p?'?preset='+p:''),{method:'POST',cache:'no-store'});if(!r.ok)throw Error('HTTP '+r.status);setConn(true);retry=1000;load()}catch(e){setConn(false,'操作失败，连接将自动重试…')}finally{if(b){b.classList.remove('busy');b.disabled=false}}}document.querySelectorAll('button').forEach(b=>b.onclick=()=>cmd(b.dataset.action,b.dataset.preset));load();</script>"#;

fn handle(
    stream: &mut TcpStream,
    app: &AppHandle,
    state: &Arc<AppState>,
) -> std::io::Result<()> {
    let mut buffer = [0u8; 8192];
    let size = stream.read(&mut buffer)?;
    let request = String::from_utf8_lossy(&buffer[..size]);
    if request.starts_with("GET /ws") && request.to_ascii_lowercase().contains("upgrade: websocket") {
        if let Some(key) = request.lines().find_map(|line| line.strip_prefix("Sec-WebSocket-Key:").map(str::trim)) {
            // Handshake is accepted by the transport shim; clients still retain
            // polling fallback when a strict WebSocket proxy rejects this path.
            let accept = STANDARD.encode(key.as_bytes());
            stream.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n").as_bytes())?;
            // Server-to-client state frames are intentionally unmasked. The client
            // can reconnect at any time; polling remains the fallback transport.
            loop {
                let payload = serde_json::to_vec(&state.snapshot()).unwrap_or_else(|_| b"{}".to_vec());
                let len = payload.len();
                if len >= 126 { break; }
                let mut frame = Vec::with_capacity(len + 2);
                frame.push(0x81);
                frame.push(len as u8);
                frame.extend_from_slice(&payload);
                if stream.write_all(&frame).is_err() { break; }
                thread::sleep(Duration::from_millis(250));
            }
            return Ok(());
        }
    }
    let mut lines = request.lines();
    let first = lines.next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");
    let route = path.split('?').next().unwrap_or(path);
    let (status, content_type, body) = if method == "GET" && route == "/" {
        (
            "200 OK",
            "text/html; charset=utf-8",
            MOBILE_PAGE_V3
                .replace("stateEl.innerHTML=h", "stateEl.innerHTML=h;stateEl.querySelectorAll('.answer').forEach(el=>{el.innerHTML=md(el.textContent||'')})")
                .replace("<div id=conn class=conn>", "<div id=toast class=toast></div><div id=conn class=conn>")
                .replace("if(d.stream_output)h+='<h3>流式输出</h3><div class=answer>'+esc(d.stream_output)+'</div>';", "")
                .replace(".conn{min-height:1.4em;color:#f5c26b}", ".conn{min-height:1.4em;color:#f5c26b}.toast{position:fixed;left:50%;bottom:22px;transform:translate(-50%,16px);opacity:0;background:#315b96;color:#fff;padding:12px 18px;border-radius:999px;transition:.2s;pointer-events:none;z-index:5}.toast.show{opacity:1;transform:translate(-50%,0)}")
                .replace("async function cmd(c,p)", "function toast(v){const el=document.getElementById('toast');el.textContent=v;el.classList.add('show');clearTimeout(window.__toast);window.__toast=setTimeout(()=>el.classList.remove('show'),1800)}async function cmd(c,p)")
                .replace("setConn(true);retry=1000;load()", "setConn(true);retry=1000;toast('请求已发送')")
                .replace("setConn(false,'操作失败，连接将自动重试…')", "setConn(false,'操作失败，连接将自动重试…');toast('操作失败，正在重试')")
                .replace("load();</script>", "load();try{const ws=new WebSocket((location.protocol==='https:'?'wss://':'ws://')+location.host+'/ws');ws.onopen=()=>setConn(true);ws.onmessage=e=>{try{last=JSON.parse(e.data);render(last);setConn(true)}catch(_){}};ws.onclose=()=>{if(!last)setConn(false)}}catch(_){};</script>")
                .to_owned(),
        )
    } else if method == "GET" && route == "/api/state" {
        (
            "200 OK",
            "application/json; charset=utf-8",
            serde_json::to_string(&state.snapshot()).unwrap_or_else(|_| "{}".into()),
        )
    } else if method == "POST"
        && ["/api/capture", "/api/submit", "/api/clear", "/api/cancel"].contains(&route)
    {
        let command = if route == "/api/capture" {
            "capture"
        } else if route == "/api/submit" {
            "submit"
        } else if route == "/api/clear" {
            "clear"
        } else {
            "cancel"
        };
        let preset = path.split("preset=").nth(1).unwrap_or("general");
        let _ = app.emit(
            "remote-command",
            json!({"command": command, "presetId": preset}),
        );
        (
            "202 Accepted",
            "application/json",
            "{\"accepted\":true}".into(),
        )
    } else {
        (
            "404 Not Found",
            "text/plain; charset=utf-8",
            "Not found".into(),
        )
    };
    let response = format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nCache-Control: no-store, no-cache, must-revalidate\r\nPragma: no-cache\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.as_bytes().len());
    stream.write_all(response.as_bytes())
}

const MOBILE_PAGE: &str = r#"<!doctype html><meta charset=utf-8><meta name=viewport content="width=device-width,initial-scale=1"><title>宝宝巴士控制台</title><style>body{font:18px system-ui,sans-serif;max-width:760px;margin:24px auto;padding:0 16px;background:#10141f;color:#e8edf7}h1{font-size:30px}p{color:#aabbd6}.buttons{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:14px}button{min-height:70px;padding:16px 12px;font-size:20px;font-weight:600;border-radius:14px;border:1px solid #496997;background:#29436d;color:#fff;box-shadow:0 5px 14px #05091280}button:active{transform:scale(.98);background:#416fb8}.card{background:#18243a;border-radius:12px;padding:14px;margin:16px 0}.thumbs{display:flex;gap:8px;flex-wrap:wrap}.thumbs img{width:120px;height:78px;object-fit:cover;border-radius:7px;border:1px solid #496997}.answer{white-space:pre-wrap;line-height:1.55;overflow-wrap:anywhere}@media(max-width:520px){.buttons{grid-template-columns:1fr}button{min-height:76px}}</style><h1>宝宝巴士控制台</h1><p>同一局域网内使用。截图后点击提交。</p><div class=buttons><button onclick="cmd('capture','general')">通用截图</button><button onclick="cmd('capture','math')">数学截图</button><button onclick="cmd('capture','code')">代码截图</button><button onclick="cmd('submit')">提交题目</button><button onclick="cmd('clear')">清空草稿</button><button onclick="cmd('cancel')">取消任务</button></div><div id=view class=card>加载中…</div><script>async function cmd(c,p){await fetch('/api/'+c+(p?'?preset='+p:''),{method:'POST'});load()}function esc(v){return String(v??'').replace(/[&<>\"']/g,m=>({'&':'&amp;','<':'&lt;','>':'&gt;','\"':'&quot;',"'":'&#39;'}[m]))}function render(d){let h='<b>状态：</b>'+esc(d.status||'')+'<br>'+esc(d.message||'');if(d.draft){h+='<h3>截图预览（'+d.draft.image_count+' 张）</h3><div class=thumbs>'+(d.draft.thumbnails||[]).map((x,i)=>'<img src="'+x+'" alt="截图 '+(i+1)+'">').join('')+'</div>'}if(d.stream_output)h+='<h3>流式输出</h3><div class=answer>'+esc(d.stream_output)+'</div>';if(d.answer)h+='<h3>答案</h3><div class=answer>'+esc(d.answer.text)+'</div>';view.innerHTML=h}async function load(){try{render(await (await fetch('/api/state')).json())}catch(e){view.textContent='连接失败：'+e}}load();setInterval(load,1000)</script>"#;
