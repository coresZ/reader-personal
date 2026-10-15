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
  if (!types) {
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
    BigInteger.prototype.bitLength = function () { var n = this.value < 0n ? ~this.value : this.value; return n === 0n ? 0 : n.toString(2).length; };
    BigInteger.prototype.testBit = function (n) { return !!(this.value & (1n << BigInt(n))); };
    BigInteger.prototype.toByteArray = function () {
      var count = Math.max(1, Math.ceil((this.bitLength() + 1) / 8)), value = BigInt.asUintN(count * 8, this.value), out = new Array(count);
      for (var i = count - 1; i >= 0; i--) { out[i] = Number(BigInt.asIntN(8, value)); value >>= 8n; }
      return out;
    };
    BigInteger.valueOf = function (v) { return new BigInteger(v); };
    BigInteger.ZERO = new BigInteger(0); BigInteger.ONE = new BigInteger(1); BigInteger.TEN = new BigInteger(10);

    function JavaString(value, charset, length, encoding) {
      var text;
      if (Array.isArray(value) || ArrayBuffer.isView(value)) {
        var bytes = array(value);
        if (typeof charset === 'number') { bytes = bytes.slice(charset, charset + length); charset = encoding; }
        text = native('decode', {data: bytes, charset: charset || 'UTF-8'});
      } else text = value === undefined || value === null ? '' : String(value);
      var result = new String(text);
      Object.setPrototypeOf(result, JavaString.prototype);
      return result;
    }
    JavaString.prototype = Object.create(String.prototype);
    JavaString.prototype.constructor = JavaString;
    JavaString.prototype.getBytes = function (charset) { return native('encode', {text: String(this), charset: charset || 'UTF-8'}); };
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
    // `Packages.java.util.Collections`: 源脚本(如「源仓库」订阅源的 header 脚本)用
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

    types = g.__readerJavaTypes = {
      math:{BigInteger:BigInteger},
      lang:{String:JavaString, System:{currentTimeMillis:function () { return Date.now(); }, arraycopy:arraycopy}, Thread:{sleep:function (ms) { g.__sleep(Number(ms)); }}},
      util:{Arrays:Arrays, Base64:Base64, Collections:Collections, zip:{GZIPInputStream:GZIPInputStream}},
      security:{MessageDigest:MessageDigest, SecureRandom:SecureRandom},
      io:{ByteArrayInputStream:ByteArrayInputStream, ByteArrayOutputStream:ByteArrayOutputStream},
      crypto:{Cipher:Cipher, spec:{SecretKeySpec:SecretKeySpec, IvParameterSpec:IvParameterSpec}}
    };
  }
  g.java.math = types.math;
  Object.assign(g.java.lang, types.lang);
  Object.assign(g.java.util, types.util);
  g.java.security = types.security;
  g.java.io = types.io;
  g.javax = {crypto:types.crypto};
  g.Packages.javax = g.javax;
})(globalThis);
