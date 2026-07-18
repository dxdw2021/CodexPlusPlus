const http = require('http');
    const { spawn } = require('child_process');
    const port = 9229;
    const injectorScript = "D:\\DEV\\tool\\AI\\CodexPlusPlus-main\\assets\\dream-skin\\scripts\\injector.mjs";

    function sleep(ms) { return new Promise(r => setTimeout(r, ms)); }
    function fetchJson(url) {
      return new Promise((resolve, reject) => {
        http.get(url, res => {
          let data = '';
          res.on('data', c => data += c);
          res.on('end', () => { try { resolve(JSON.parse(data)); } catch(e) { reject(e); } });
        }).on('error', reject);
      });
    }

    (async () => {
      for (let i = 0; i < 120; i++) {
        try {
          const version = await fetchJson(`http://127.0.0.1:${port}/json/version`);
          const browserId = version.webSocketDebuggerUrl?.match(/\/devtools\/browser\/([A-Za-z0-9._-]+)$/)?.[1];
          if (browserId) {
            const child = spawn(process.execPath, [injectorScript,
              '--watch', '--port', String(port), '--browser-id', browserId
            ], { stdio: 'ignore', detached: true });
            child.on('error', (err) => {
              console.error('injector spawn failed:', err.message);
              process.exit(2);
            });
            child.unref();
            // 给 injector 一点时间启动
            await sleep(2000);
            process.exit(0);
          }
        } catch(e) {
          console.error('helper error:', e?.message || e);
        }
        await sleep(1000);
      }
      process.exit(1);
    })();
    