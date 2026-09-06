#!/usr/bin/env python3
"""Real Chromium + disposable Admin HTTP + synthetic Provider. No production input."""
import json, os, subprocess, tempfile, time
from pathlib import Path
from playwright.sync_api import sync_playwright, expect

root=Path(__file__).resolve().parents[2]
output=Path(os.environ.get('MCP_VAULT_E2E_OUTPUT_DIR',str(root/'target/memory-review/browser-e2e')))
output.mkdir(parents=True,exist_ok=True)
with tempfile.TemporaryDirectory(prefix='mcp-vault-browser-') as temporary:
    manifest=Path(temporary)/'manifest.json'
    environment=dict(os.environ,MCP_VAULT_FIXTURE_MANIFEST=str(manifest),MCP_VAULT_FIXTURE_MEMORY_ADMIN='1')
    with (output/'fixture.log').open('w') as log:
        fixture=subprocess.Popen([str(root/'target/debug/mcp-vault-fixture')],cwd=root,env=environment,stdout=log,stderr=subprocess.STDOUT)
        try:
            for _ in range(1200):
                if manifest.exists(): break
                if fixture.poll() is not None: raise RuntimeError('fixture exited; inspect fixture.log')
                time.sleep(.1)
            config=json.loads(manifest.read_text())
            origin=config['admin_url'].split('/api/')[0]
            with sync_playwright() as p:
                browser=p.chromium.launch(headless=True)
                page=browser.new_page()
                requests=[]
                page.on('request',lambda request: requests.append((request.method,request.url)) if '/api/v1/' in request.url else None)
                page.on('dialog',lambda dialog:dialog.accept())
                page.set_default_timeout(15000)
                try:
                    page.goto(origin)
                    page.get_by_label('用户名',exact=True).fill('browser-admin')
                    page.get_by_label('密码',exact=True).fill('isolated-browser-password-123')
                    # Let the fixture's initial outbox/embedding jobs drain before login.
                    page.wait_for_timeout(2000)
                    for login_attempt in range(3):
                        page.get_by_role('button',name='登录管理端',exact=True).click()
                        page.wait_for_timeout(1000)
                        if page.get_by_role('button',name='记忆',exact=True).count(): break
                        if not page.get_by_text('请求失败（authentication_unavailable），请稍后重试。',exact=True).count(): break

                    page.get_by_role('button',name='记忆',exact=True).first.click()
                    expect(page.get_by_text('检索效果诊断（可选）',exact=True)).to_be_visible()
                    assert not any('/semantic-calibration/run' in url for _,url in requests),'page GET must not run calibration'
                    page.wait_for_timeout(1000)
                    (output/'loaded-debug.txt').write_text(page.locator('body').inner_text())
                    expect(page.get_by_text('当前记忆向量覆盖完整，可用于语义检索；相似度不代表资料一定回答了问题。',exact=True)).to_be_visible()
                    expect(page.get_by_text('内置基准通过',exact=True)).to_have_count(0)
                    # U3: real worker computation, metrics are never submitted by UI.
                    page.get_by_role('button',name='运行／重试评测',exact=True).first.click()
                    for _ in range(60):
                        if page.get_by_text('内置基准通过',exact=True).count(): break
                        page.wait_for_timeout(300)
                        page.get_by_role('button',name='刷新状态',exact=True).click()
                    expect(page.get_by_text('内置基准通过',exact=True)).to_be_visible()
                    assert not any(method=='PUT' and url.endswith('/semantic-calibration') for method,url in requests)
                    page.get_by_text('查看最近执行结果',exact=True).first.click()
                    assert page.evaluate('document.documentElement.scrollWidth <= window.innerWidth'), 'expanded calibration report must not widen the page'
                    # U6: server pagination, not a static count.
                    page.get_by_role('button',name='加载更多记忆',exact=True).click()
                    expect(page.get_by_text('长期记忆（已加载 53 条）',exact=True)).to_be_visible()
                    # U5: edit an explicit record through revision-aware PATCH.
                    page.get_by_role('button',name='编辑显式记忆',exact=False).first.click(force=True)
                    page.screenshot(path=str(output/'editor-debug.png'),full_page=True)
                    (output/'editor-debug.txt').write_text(page.locator('body').inner_text())
                    page.locator('form[aria-label="编辑显式记忆"] textarea').fill('Browser edited explicit assertion')
                    page.get_by_role('button',name='保存修改',exact=True).click()
                    expect(page.get_by_text('Browser edited explicit assertion',exact=True)).to_be_visible()
                    assert any(method=='PATCH' and '/memories/' in url for method,url in requests)
                    # U1/U2: actual legacy migration preflight and fingerprint apply.
                    page.get_by_role('button',name='迁移预检',exact=True).click()
                    page.get_by_label('迁移确认字符串',exact=True).fill('MIGRATE_MEMORY_V2_1')
                    page.get_by_role('button',name='执行已预检的迁移',exact=True).click()
                    expect(page.get_by_text('迁移已完成。',exact=True)).to_be_visible()
                    # U4: delete the only derived item; its paused empty source survives refresh.
                    item=page.locator('article.record-item').filter(has=page.get_by_text('Synthetic team completed the local exercise.',exact=True)).last
                    item.get_by_role('button',name='删除当前记忆',exact=False).click()
                    page.get_by_role('button',name='刷新状态',exact=True).click()
                    expect(page.get_by_text('browser-source.md',exact=True)).to_be_visible()
                    page.get_by_role('button',name='恢复来源提取',exact=True).click()
                    expect(page.get_by_text('来源恢复已提交，后台完成后自动提取。',exact=True)).to_be_visible()
                    page.screenshot(path=str(output/'memory-admin.png'),full_page=True)
                    page.get_by_role('button',name='AI 服务',exact=True).first.click()
                    page.get_by_text('高级：摘要、Embedding 与重排模型',exact=True).click()
                    expect(page.get_by_text('笔记语义分块（可选）',exact=True)).to_have_count(0)
                    (output/'result.json').write_text(json.dumps({'result':'passed','backend':'real_admin_http','provider':'local_synthetic_contract','flows':['U1','U2','U3','U4','U5','U6_pagination','rule_chunking_no_model_control'],'production_data':False},indent=2)+'\n')
                except Exception:
                    (output/'failure.txt').write_text(page.locator('body').inner_text())
                    page.screenshot(path=str(output/'failure.png'),full_page=True,timeout=10000)
                    raise
                browser.close()
                print('PASS: real browser Admin U1-U5 and pagination; local synthetic Provider only')
        finally:
            fixture.terminate()
            try: fixture.wait(timeout=10)
            except subprocess.TimeoutExpired: fixture.kill();fixture.wait()
