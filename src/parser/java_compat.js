(function (g) {
  function native(op, data) {
    var result = JSON.parse(g.__binary(op, JSON.stringify(data)));
    if (result.error) throw new Error(result.error);
    return result.value;
  }
  function array(value) { return Array.prototype.slice.call(value || []).map(function (v) { return (Number(v) << 24) >> 24; }); }
  function size(n) {
    n = Number(n);
    if (!Number.isInteger(n) || n < 0 || n > 16777216) throw new RangeError('Invalid array length');
    return n;
  }

  // Keep constructors stable across rule evaluations: jsLib may retain aliases
  // and use instanceof against them, while the native host functions refresh.
  var types = g.__readerJavaTypes;
  // 必须每次求值都跑: 类型表跨次复用(构造器要稳定), 但本轮新增的命名空间
  // (`java.nio` / `java.net` 等)只有执行到这里才能补进旧表。
  {
    types = types || {};
    // `new BigInteger(numBits, SecureRandom)`: Java 的随机大数构造(书源常用它
    // 生成 DH 私钥)。radix 位置拿到的是 SecureRandom 实例。
    function randomBigInt(bits) {
      bits = Number(bits) || 0;
      if (bits < 0) throw new Error('Invalid bit length');
      if (bits === 0) return 0n;
      var data = native('random', { length: Math.ceil(bits / 8) }), number = 0n;
      for (var i = 0; i < data.length; i++) number = (number << 8n) | BigInt(data[i] & 255);
      var excess = BigInt(data.length * 8 - bits);
      return excess > 0n ? number >> excess : number;
    }
    function big(value, radix) {
      if (value instanceof BigInteger) return value.value;
      if (Array.isArray(value) || ArrayBuffer.isView(value)) {
        var data = array(value), number = 0n;
        for (var i = 0; i < data.length; i++) number = (number << 8n) | BigInt(data[i] & 255);
        return data.length && data[0] < 0 ? number - (1n << BigInt(data.length * 8)) : number;
      }
      if (radix === undefined || Number(radix) === 10) return BigInt(value);
      var base = Number(radix), text = String(value).toLowerCase(), sign = 1n, number = 0n;
      if (base < 2 || base > 36) throw new RangeError('Invalid radix');
      if (text[0] === '-' || text[0] === '+') { sign = text[0] === '-' ? -1n : 1n; text = text.slice(1); }
      if (!text.length) throw new Error('Invalid integer');
      for (var i = 0; i < text.length; i++) {
        var digit = '0123456789abcdefghijklmnopqrstuvwxyz'.indexOf(text[i]);
        if (digit < 0 || digit >= base) throw new Error('Invalid integer');
        number = number * BigInt(base) + BigInt(digit);
      }
      return sign * number;
    }
    function BigInteger(value, radix) {
      if (!(this instanceof BigInteger)) return new BigInteger(value, radix);
      if (typeof value === 'number' && (Array.isArray(radix) || ArrayBuffer.isView(radix))) {
        var data = array(radix), number = 0n;
        for (var i = 0; i < data.length; i++) number = (number << 8n) | BigInt(data[i] & 255);
        this.value = BigInt(value) * number;
      } else if (radix instanceof SecureRandom) {
        this.value = randomBigInt(value);
      } else this.value = big(value, radix);
    }
    BigInteger.prototype.toString = function (radix) { return this.value.toString(radix === undefined ? 10 : Number(radix)); };
    BigInteger.prototype.toJSON = function () { return this.toString(); };
    BigInteger.prototype.intValue = function () { return Number(BigInt.asIntN(32, this.value)); };
    BigInteger.prototype.longValue = function () { return Number(BigInt.asIntN(64, this.value)); };
    BigInteger.prototype.byteValue = function () { return Number(BigInt.asIntN(8, this.value)); };
    BigInteger.prototype.signum = function () { return this.value < 0n ? -1 : this.value > 0n ? 1 : 0; };
    BigInteger.prototype.compareTo = function (v) { var other = big(v); return this.value < other ? -1 : this.value > other ? 1 : 0; };
    BigInteger.prototype.equals = function (v) { return v instanceof BigInteger && this.value === v.value; };
    BigInteger.prototype.add = function (v) { return new BigInteger(this.value + big(v)); };
    BigInteger.prototype.subtract = function (v) { return new BigInteger(this.value - big(v)); };
    BigInteger.prototype.multiply = function (v) { return new BigInteger(this.value * big(v)); };
    BigInteger.prototype.divide = function (v) { return new BigInteger(this.value / big(v)); };
    BigInteger.prototype.remainder = function (v) { return new BigInteger(this.value % big(v)); };
    BigInteger.prototype.mod = function (v) { var modulus = big(v); if (modulus <= 0n) throw new Error('Nonpositive modulus'); return new BigInteger((this.value % modulus + modulus) % modulus); };
    BigInteger.prototype.and = function (v) { return new BigInteger(this.value & big(v)); };
    BigInteger.prototype.or = function (v) { return new BigInteger(this.value | big(v)); };
    BigInteger.prototype.xor = function (v) { return new BigInteger(this.value ^ big(v)); };
    BigInteger.prototype.not = function () { return new BigInteger(~this.value); };
    BigInteger.prototype.shiftLeft = function (n) { return new BigInteger(this.value << BigInt(n)); };
    BigInteger.prototype.shiftRight = function (n) { return new BigInteger(this.value >> BigInt(n)); };
    BigInteger.prototype.negate = function () { return new BigInteger(-this.value); };
    BigInteger.prototype.abs = function () { return new BigInteger(this.value < 0n ? -this.value : this.value); };
    BigInteger.prototype.pow = function (n) { return new BigInteger(this.value ** BigInt(n)); };
    // `modPow`: 源脚本的 DH/签名计算。
    BigInteger.prototype.modPow = function (exponent, modulus) {
      var e = big(exponent), m = big(modulus);
      if (m <= 0n) throw new Error('Nonpositive modulus');
      if (e < 0n) throw new Error('Unsupported negative exponent');
      var result = 1n, base = ((this.value % m) + m) % m;
      while (e > 0n) {
        if (e & 1n) result = (result * base) % m;
        base = (base * base) % m;
        e >>= 1n;
      }
      return new BigInteger(result);
    };
    BigInteger.prototype.bitLength = function () { var n = this.value < 0n ? ~this.value : this.value; return n === 0n ? 0 : n.toString(2).length; };
    BigInteger.prototype.testBit = function (n) { return !!(this.value & (1n << BigInt(n))); };
    BigInteger.prototype.toByteArray = function () {
      var count = Math.max(1, Math.ceil((this.bitLength() + 1) / 8)), value = BigInt.asUintN(count * 8, this.value), out = new Array(count);
      for (var i = count - 1; i >= 0; i--) { out[i] = Number(BigInt.asIntN(8, value)); value >>= 8n; }
      return out;
    };
    BigInteger.valueOf = function (v) { return new BigInteger(v); };
    BigInteger.ZERO = new BigInteger(0); BigInteger.ONE = new BigInteger(1); BigInteger.TEN = new BigInteger(10);

    // Charset 实例/名字统一成宿主能识别的标签字符串。
    function charsetLabel(charset) {
      if (charset === undefined || charset === null) return 'UTF-8';
      if (charset instanceof Charset) return charset.name();
      if (typeof charset === 'object' && charset.__name) return String(charset.__name);
      return String(charset);
    }
    // `ISO-8859-1` 在 web 编码表里被映到 windows-1252, 0x80..0x9F 段与真实字节
    // 不一致。源脚本用这个字符集在**字节**与字符串之间做无损转换, 因此单独走
    // 纯 JS 的逐字节映射, 不经过宿主的编码表。
    function isLatin1(label) {
      return /^(iso[-_ ]?8859[-_ ]?1|latin[-_ ]?1)$/i.test(String(label).trim());
    }
    function latin1Decode(bytes) {
      var out = '';
      for (var i = 0; i < bytes.length; i++) out += String.fromCharCode(bytes[i] & 255);
      return out;
    }
    function latin1Encode(text) {
      var bytes = [];
      for (var i = 0; i < text.length; i++) bytes.push(text.charCodeAt(i) & 255);
      return bytes;
    }
    function JavaString(value, charset, length, encoding) {
      var text;
      if (Array.isArray(value) || ArrayBuffer.isView(value)) {
        var bytes = array(value);
        if (typeof charset === 'number') { bytes = bytes.slice(charset, charset + length); charset = encoding; }
        var label = charsetLabel(charset);
        text = isLatin1(label) ? latin1Decode(bytes) : native('decode', {data: bytes, charset: label});
      } else text = value === undefined || value === null ? '' : String(value);
      var result = new String(text);
      Object.setPrototypeOf(result, JavaString.prototype);
      return result;
    }
    JavaString.prototype = Object.create(String.prototype);
    JavaString.prototype.constructor = JavaString;
    JavaString.prototype.getBytes = function (charset) {
      var label = charsetLabel(charset);
      return isLatin1(label) ? latin1Encode(String(this)) : native('encode', {text: String(this), charset: label});
    };
    JavaString.prototype.equals = function (v) { return String(this) === String(v); };

    var Arrays = {
      copyOf: function (value, length) { return Arrays.copyOfRange(value, 0, size(length)); },
      copyOfRange: function (value, start, end) {
        start = size(start); end = size(end);
        if (start > value.length || start > end) throw new RangeError('Invalid array range');
        var out = array(value).slice(start, end);
        while (out.length < end - start) out.push(0);
        return out;
      }
    };
    function arraycopy(src, start, dest, offset, length) {
      start = size(start); offset = size(offset); length = size(length);
      if (start + length > src.length || offset + length > dest.length) throw new RangeError('Invalid copy range');
      var copy = array(src).slice(start, start + length);
      for (var i = 0; i < length; i++) dest[offset + i] = copy[i];
    }
    var Base64 = {
      getEncoder: function () { return {encodeToString: function (v) { return native('base64Encode', {data:array(v)}); }}; },
      getDecoder: function () { return {decode: function (v) { return native('base64Decode', {text:String(v)}); }}; }
    };
    // `Packages.java.util.UUID.randomUUID()`: 源脚本用它生成设备指纹字段,
    // 与 legado 的 java.randomUUID() 同源。
    var UUID = {
      randomUUID: function () { return String(g.java.uuid()); },
      fromString: function (text) { return String(text); },
      toString: function () { return String(g.java.uuid()); }
    };
    // `Packages.java.nio.charset.Charset.forName('ISO-8859-1')`: 源脚本用它把
    // 二进制字节与 JS 字符串互转。只需承载名字供
    // JavaString 构造器识别，不需要真正的编码表。
    var charsetInstances = {};
    // Rhino 里 `Packages.java.nio.charset.Charset` 就是类本身, 静态方法挂在类上。
    function Charset(name) { this.__name = String(name || 'UTF-8'); }
    Charset.prototype.name = function () { return this.__name; };
    Charset.prototype.toString = function () { return this.__name; };
    Charset.forName = function (name) {
      var key = String(name).toUpperCase();
      if (!charsetInstances[key]) charsetInstances[key] = new Charset(key);
      return charsetInstances[key];
    };
    Charset.isSupported = function () { return true; };
    Charset.defaultCharset = function () { return Charset.forName('UTF-8'); };
    // `Packages.java.util.Collections`: 源脚本(如聚合类源的 header 脚本)用
    // `Collections.reverse(list)` 就地反转载荷列表, 缺了它会抛
    // "cannot read property 'reverse' of undefined" 并中断整段脚本。
    //
    // 只交换**索引位置**, 绝不用 `array()` 重建列表: 入参常是 jsoup 包装过的
    // Elements(元素带 `attr()`/`text()` 方法), 经 `array()` 转换会退化成纯数据,
    // 之后 `char.attr('href')` 就成了 "not a function"。
    // 与 Java 一致地返回 undefined 并修改入参。
    function swapInPlace(list) {
      var len = list.length >>> 0;
      for (var i = 0, j = len - 1; i < j; i++, j--) {
        var tmp = list[i]; list[i] = list[j]; list[j] = tmp;
      }
      return len;
    }
    var Collections = {
      reverse: function (list) {
        if (list === null || list === undefined) throw new TypeError('Collections.reverse: list is null');
        swapInPlace(list);
        return undefined;
      },
      sort: function (list) {
        if (list === null || list === undefined) throw new TypeError('Collections.sort: list is null');
        // 只对可比较的标量列表排序; 同样按索引写回, 保留元素对象本身。
        var len = list.length >>> 0;
        var order = [];
        for (var i = 0; i < len; i++) order.push(list[i]);
        order.sort(function (a, b) { return a < b ? -1 : a > b ? 1 : 0; });
        for (var k = 0; k < len; k++) list[k] = order[k];
        return undefined;
      },
      shuffle: function (list) {
        if (list === null || list === undefined) throw new TypeError('Collections.shuffle: list is null');
        var len = list.length >>> 0;
        for (var i = len - 1; i > 0; i--) {
          var j = Math.floor(Math.random() * (i + 1));
          var tmp = list[i]; list[i] = list[j]; list[j] = tmp;
        }
        return undefined;
      },
      emptyList: function () { return []; },
      singletonList: function (value) { return [value]; }
    };
    function SecureRandom() {}
    SecureRandom.prototype.nextBytes = function (target) { arraycopy(native('random', {length:target.length}), 0, target, 0, target.length); };
    SecureRandom.prototype.generateSeed = function (length) { return native('random', {length:size(length)}); };
    var MessageDigest = {getInstance: function (algorithm) {
      var pending = [];
      return {
        update: function (value) { pending = pending.concat(typeof value === 'number' ? [value] : array(value)); },
        digest: function (value) { var data = pending.concat(value === undefined ? [] : array(value)); pending = []; return native('digest', {algorithm:String(algorithm), data:data}); },
        reset: function () { pending = []; }
      };
    }};
    function SecretKeySpec(value, algorithm) { this.bytes = array(value); this.algorithm = String(algorithm); }
    SecretKeySpec.prototype.getEncoded = function () { return this.bytes.slice(); };
    function IvParameterSpec(value) { this.bytes = array(value); }
    var Cipher = {ENCRYPT_MODE:1, DECRYPT_MODE:2, getInstance:function (transformation) {
      var key, iv = [], encrypt, pending = [];
      return {
        init: function (mode, k, v) { if (mode !== 1 && mode !== 2) throw new Error('Unsupported cipher mode'); encrypt = mode === 1; key = k.bytes; iv = v ? v.bytes : []; pending = []; },
        update: function (data) { pending = pending.concat(array(data)); return []; },
        doFinal: function (data) { if (!key) throw new Error('Cipher not initialized'); var input = pending.concat(data === undefined ? [] : array(data)); pending = []; return native('cipher', {transformation:String(transformation), encrypt:encrypt, key:key, iv:iv, data:input}); }
      };
    }};
    function ByteArrayInputStream(data) { this.bytes = array(data); this.position = 0; }
    ByteArrayInputStream.prototype.read = function (target, offset, length) {
      if (this.position >= this.bytes.length) return -1;
      if (target === undefined) return this.bytes[this.position++] & 255;
      offset = offset === undefined ? 0 : size(offset);
      length = length === undefined ? target.length - offset : size(length);
      if (offset + length > target.length) throw new RangeError('Invalid read range');
      var count = Math.min(length, this.bytes.length - this.position);
      arraycopy(this.bytes, this.position, target, offset, count); this.position += count; return count;
    };
    ByteArrayInputStream.prototype.close = function () {};
    function ByteArrayOutputStream() { this.bytes = []; }
    ByteArrayOutputStream.prototype.write = function (data, offset, length) {
      if (typeof data === 'number') this.bytes.push((data << 24) >> 24);
      else { offset = offset || 0; var values = array(data); this.bytes = this.bytes.concat(values.slice(offset, length === undefined ? undefined : offset + length)); }
    };
    ByteArrayOutputStream.prototype.toByteArray = function () { return this.bytes.slice(); };
    ByteArrayOutputStream.prototype.close = function () {};
    function GZIPInputStream(input) { ByteArrayInputStream.call(this, native('gunzip', {data:input.bytes.slice(input.position)})); }
    GZIPInputStream.prototype = Object.create(ByteArrayInputStream.prototype);
    function GZIPOutputStream(output) {
      this.out = output;
      this.pending = [];
      this.closed = false;
    }
    GZIPOutputStream.prototype.write = function (data, offset, length) {
      if (typeof data === 'number') this.pending.push(data & 255);
      else {
        offset = offset || 0;
        var values = array(data);
        this.pending = this.pending.concat(values.slice(offset, length === undefined ? undefined : offset + length));
      }
    };
    GZIPOutputStream.prototype.close = function () {
      if (this.closed) return;
      this.closed = true;
      // Java 的 GZIPOutputStream 在 close 时写出完整 gzip 流; 目标流按 Java 语义
      // 接收 int 数组, 因此这里保持数组写入(不用 latin1 字符串)。
      this.out.write(native('gzip', { data: this.pending }));
      this.pending = [];
    };
    GZIPOutputStream.prototype.finish = GZIPOutputStream.prototype.close;
    // `Packages.java.io.BufferedReader(new InputStreamReader(stream,'UTF-8'))`:
    // 源脚本用它逐行读取响应文本。这里按 ByteArrayInputStream 的字节缓冲读出
    // 全部内容再切行, readLine 逐行返回。
    function InputStreamReader(stream, charset) {
      this.stream = stream;
      this.charset = charset || 'UTF-8';
      this.text = null;
      this.lines = null;
      this.index = 0;
    }
    InputStreamReader.prototype.__text = function () {
      if (this.text !== null) return this.text;
      var bytes = [];
      if (this.stream instanceof ByteArrayInputStream) bytes = this.stream.bytes.slice(this.stream.position);
      else if (this.stream && typeof this.stream.read === 'function') {
        var chunk = [0];
        while (this.stream.read(chunk, 0, 1) > 0) bytes.push(chunk[0] & 255);
      } else bytes = array(this.stream);
      this.text = native('decode', { data: bytes, charset: this.charset });
      return this.text;
    };
    InputStreamReader.prototype.readLine = function () {
      if (this.lines === null) this.lines = String(this.__text()).split(/\r\n|\r|\n/);
      return this.index < this.lines.length ? this.lines[this.index++] : null;
    };
    InputStreamReader.prototype.read = function () {
      var text = this.__text();
      if (this.index >= text.length) return -1;
      return text.charCodeAt(this.index++);
    };
    InputStreamReader.prototype.close = function () { if (this.stream && this.stream.close) this.stream.close(); };
    function BufferedReader(reader) {
      this.reader = reader instanceof InputStreamReader ? reader : new InputStreamReader(reader, 'UTF-8');
    }
    BufferedReader.prototype.readLine = function () { return this.reader.readLine(); };
    BufferedReader.prototype.read = function () { return this.reader.read(); };
    BufferedReader.prototype.close = function () { this.reader.close(); };

    // `Packages.java.net.URL(url).openConnection()`: 源脚本用它发送带自定义
    // 请求头的二进制 POST 并读回响应。底层复用宿主 `__http_bytes` 原语:
    // 请求体按 latin1 还原成原始字节, 响应体按 latin1 原样带回。
    //
    // 这里的 `this` 指向每次 openConnection 新建的连接对象, 与其他 Java 互操作
    // 类的"实例方法"写法一致。
    function URLImpl(url) { this.__url = String(url); }
    URLImpl.prototype.toString = function () { return this.__url; };
    URLImpl.prototype.getProtocol = function () { var m = /^([a-z][a-z0-9+.-]*):/i.exec(this.__url); return m ? m[1].toLowerCase() : ''; };
    URLImpl.prototype.getHost = function () { var m = /^[a-z][a-z0-9+.-]*:\/\/([^/:?#]+)/i.exec(this.__url); return m ? m[1] : ''; };
    URLImpl.prototype.getPath = function () { var m = /^[a-z][a-z0-9+.-]*:\/\/[^/]*(\/[^?#]*)?/i.exec(this.__url); return m && m[1] ? m[1] : ''; };
    URLImpl.prototype.openConnection = function () { return new UrlConnection(this.__url); };
    URLImpl.prototype.openStream = function () { return this.openConnection().getInputStream(); };

    function UrlConnection(url) {
      this.__url = String(url);
      this.__method = 'GET';
      this.__headers = {};
      this.__body = [];
      this.__response = null;
    }
    UrlConnection.prototype.setRequestMethod = function (method) { this.__method = String(method).toUpperCase(); };
    UrlConnection.prototype.setRequestProperty = function (key, value) { this.__headers[String(key)] = value == null ? '' : String(value); };
    UrlConnection.prototype.setDoOutput = function () { this.__doOutput = true; };
    UrlConnection.prototype.setDoInput = function () {};
    UrlConnection.prototype.setConnectTimeout = function () {};
    UrlConnection.prototype.setReadTimeout = function () {};
    UrlConnection.prototype.setUseCaches = function () {};
    UrlConnection.prototype.setInstanceFollowRedirects = function () {};
    UrlConnection.prototype.getOutputStream = function () {
      var connection = this;
      return {
        write: function (data, offset, length) {
          if (typeof data === 'number') connection.__body.push(data & 255);
          else {
            offset = offset || 0;
            var values = array(data);
            connection.__body = connection.__body.concat(
              values.slice(offset, length === undefined ? undefined : offset + length)
            );
          }
        },
        flush: function () {},
        close: function () {}
      };
    };
    UrlConnection.prototype.__send = function () {
      if (this.__response !== null) return this.__response;
      var body = this.__body.map(function (b) { return String.fromCharCode(b & 255); }).join('');
      this.__response = JSON.parse(g.__http_bytes(
        this.__method, this.__url, body, JSON.stringify(this.__headers)
      ));
      return this.__response;
    };
    UrlConnection.prototype.getResponseCode = function () { return Number(this.__send().code) || 0; };
    UrlConnection.prototype.getInputStream = function () {
      var response = this.__send();
      var bytes = String(response.body == null ? '' : response.body).split('').map(function (c) { return c.charCodeAt(0) & 255; });
      return new ByteArrayInputStream(bytes);
    };
    UrlConnection.prototype.getErrorStream = function () { return this.getInputStream(); };
    UrlConnection.prototype.getHeaderField = function (key) {
      var headers = this.__send().headers || {}, name = String(key).toLowerCase();
      for (var k in headers) {
        if (Object.prototype.hasOwnProperty.call(headers, k) && k.toLowerCase() === name) return headers[k];
      }
      return null;
    };
    UrlConnection.prototype.disconnect = function () {};

    // 每次求值都要重新跑一遍这个脚本(prelude 的 `java` 对象是本次新建的), 但
    // Rhino 风格的类型表要跨次复用: jsLib 可能持有 `Packages.java.math.BigInteger`
    // 这类别名, 重建构造器会让 `instanceof` 失效。因此只补新字段, 不整体替换。
    types = g.__readerJavaTypes || {};
    var nextTypes = {
      math:{BigInteger:BigInteger},
      lang:{String:JavaString, System:{currentTimeMillis:function () { return Date.now(); }, arraycopy:arraycopy}, Thread:{sleep:function (ms) { g.__sleep(Number(ms)); }}},
      util:{Arrays:Arrays, Base64:Base64, Collections:Collections, UUID:UUID, zip:{GZIPInputStream:GZIPInputStream, GZIPOutputStream:GZIPOutputStream}},
      security:{MessageDigest:MessageDigest, SecureRandom:SecureRandom},
      io:{ByteArrayInputStream:ByteArrayInputStream, ByteArrayOutputStream:ByteArrayOutputStream, InputStreamReader:InputStreamReader, BufferedReader:BufferedReader},
      net:{URL:URLImpl, HttpURLConnection:UrlConnection},
      nio:{charset:{Charset:Charset}},
      crypto:{Cipher:Cipher, spec:{SecretKeySpec:SecretKeySpec, IvParameterSpec:IvParameterSpec}}
    };
    // 已有类型保持原引用(jsLib 里的 `instanceof` 依赖它), 新类型按首次注册。
    Object.keys(nextTypes).forEach(function (namespace) {
      var current = types[namespace];
      if (!current || typeof current !== 'object') { types[namespace] = nextTypes[namespace]; return; }
      Object.keys(nextTypes[namespace]).forEach(function (name) {
        if (current[name] === undefined) current[name] = nextTypes[namespace][name];
      });
      // `util.zip` / `nio.charset` / `crypto.spec` 这类二级命名空间同样按需补齐。
      Object.keys(nextTypes[namespace]).forEach(function (name) {
        var nextValue = nextTypes[namespace][name];
        if (nextValue && typeof nextValue === 'object' && current[name] && typeof current[name] === 'object' && !Array.isArray(nextValue)) {
          Object.keys(nextValue).forEach(function (inner) {
            if (current[name][inner] === undefined) current[name][inner] = nextValue[inner];
          });
        }
      });
    });
    g.__readerJavaTypes = types;
  }
  g.java.math = types.math;
  Object.assign(g.java.lang, types.lang);
  Object.assign(g.java.util, types.util);
  g.java.security = types.security;
  g.java.io = types.io;
  g.java.net = types.net;
  g.java.nio = types.nio;
  g.javax = {crypto:types.crypto};
  g.Packages.javax = g.javax;
})(globalThis);
