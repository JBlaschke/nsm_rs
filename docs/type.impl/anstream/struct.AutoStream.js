(function() {var type_impls = {
"anstream":[["<details class=\"toggle implementors-toggle\" open><summary><section id=\"impl-AutoStream%3CS%3E\" class=\"impl\"><a class=\"src rightside\" href=\"src/anstream/auto.rs.html#31-183\">source</a><a href=\"#impl-AutoStream%3CS%3E\" class=\"anchor\">§</a><h3 class=\"code-header\">impl&lt;S&gt; <a class=\"struct\" href=\"anstream/struct.AutoStream.html\" title=\"struct anstream::AutoStream\">AutoStream</a>&lt;S&gt;<div class=\"where\">where\n    S: <a class=\"trait\" href=\"anstream/stream/trait.RawStream.html\" title=\"trait anstream::stream::RawStream\">RawStream</a>,</div></h3></section></summary><div class=\"impl-items\"><details class=\"toggle method-toggle\" open><summary><section id=\"method.new\" class=\"method\"><a class=\"src rightside\" href=\"src/anstream/auto.rs.html#62-72\">source</a><h4 class=\"code-header\">pub fn <a href=\"anstream/struct.AutoStream.html#tymethod.new\" class=\"fn\">new</a>(raw: S, choice: <a class=\"enum\" href=\"anstream/enum.ColorChoice.html\" title=\"enum anstream::ColorChoice\">ColorChoice</a>) -&gt; Self</h4></section></summary><div class=\"docblock\"><p>Runtime control over styling behavior</p>\n<h5 id=\"example\"><a class=\"doc-anchor\" href=\"#example\">§</a>Example</h5>\n<div class=\"example-wrap\"><pre class=\"rust rust-example-rendered\"><code><span class=\"comment\">// Like `AutoStream::choice` but without `NO_COLOR`, `CLICOLOR_FORCE`, `CI`\n</span><span class=\"kw\">fn </span>choice(raw: <span class=\"kw-2\">&amp;</span><span class=\"kw\">dyn </span>anstream::stream::RawStream) -&gt; anstream::ColorChoice {\n    <span class=\"kw\">let </span>choice = anstream::ColorChoice::global();\n    <span class=\"kw\">if </span>choice == anstream::ColorChoice::Auto {\n        <span class=\"kw\">if </span>raw.is_terminal() &amp;&amp; anstyle_query::term_supports_color() {\n            anstream::ColorChoice::Always\n        } <span class=\"kw\">else </span>{\n            anstream::ColorChoice::Never\n        }\n    } <span class=\"kw\">else </span>{\n        choice\n    }\n}\n\n<span class=\"kw\">let </span>stream = std::io::stdout();\n<span class=\"kw\">let </span>choice = choice(<span class=\"kw-2\">&amp;</span>stream);\n<span class=\"kw\">let </span>auto = anstream::AutoStream::new(stream, choice);</code></pre></div>\n</div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.auto\" class=\"method\"><a class=\"src rightside\" href=\"src/anstream/auto.rs.html#77-81\">source</a><h4 class=\"code-header\">pub fn <a href=\"anstream/struct.AutoStream.html#tymethod.auto\" class=\"fn\">auto</a>(raw: S) -&gt; Self</h4></section></summary><div class=\"docblock\"><p>Auto-adapt for the stream’s capabilities</p>\n</div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.choice\" class=\"method\"><a class=\"src rightside\" href=\"src/anstream/auto.rs.html#85-87\">source</a><h4 class=\"code-header\">pub fn <a href=\"anstream/struct.AutoStream.html#tymethod.choice\" class=\"fn\">choice</a>(raw: <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.reference.html\">&amp;S</a>) -&gt; <a class=\"enum\" href=\"anstream/enum.ColorChoice.html\" title=\"enum anstream::ColorChoice\">ColorChoice</a></h4></section></summary><div class=\"docblock\"><p>Report the desired choice for the given stream</p>\n</div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.always_ansi\" class=\"method\"><a class=\"src rightside\" href=\"src/anstream/auto.rs.html#92-100\">source</a><h4 class=\"code-header\">pub fn <a href=\"anstream/struct.AutoStream.html#tymethod.always_ansi\" class=\"fn\">always_ansi</a>(raw: S) -&gt; Self</h4></section></summary><div class=\"docblock\"><p>Force ANSI escape codes to be passed through as-is, no matter what the inner <code>Write</code>\nsupports.</p>\n</div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.always\" class=\"method\"><a class=\"src rightside\" href=\"src/anstream/auto.rs.html#110-126\">source</a><h4 class=\"code-header\">pub fn <a href=\"anstream/struct.AutoStream.html#tymethod.always\" class=\"fn\">always</a>(raw: S) -&gt; Self</h4></section></summary><div class=\"docblock\"><p>Force color, no matter what the inner <code>Write</code> supports.</p>\n</div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.never\" class=\"method\"><a class=\"src rightside\" href=\"src/anstream/auto.rs.html#130-133\">source</a><h4 class=\"code-header\">pub fn <a href=\"anstream/struct.AutoStream.html#tymethod.never\" class=\"fn\">never</a>(raw: S) -&gt; Self</h4></section></summary><div class=\"docblock\"><p>Only pass printable data to the inner <code>Write</code>.</p>\n</div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.into_inner\" class=\"method\"><a class=\"src rightside\" href=\"src/anstream/auto.rs.html#151-158\">source</a><h4 class=\"code-header\">pub fn <a href=\"anstream/struct.AutoStream.html#tymethod.into_inner\" class=\"fn\">into_inner</a>(self) -&gt; S</h4></section></summary><div class=\"docblock\"><p>Get the wrapped <a href=\"anstream/stream/trait.RawStream.html\" title=\"trait anstream::stream::RawStream\"><code>RawStream</code></a></p>\n</div></details><section id=\"method.is_terminal\" class=\"method\"><a class=\"src rightside\" href=\"src/anstream/auto.rs.html#161-168\">source</a><h4 class=\"code-header\">pub fn <a href=\"anstream/struct.AutoStream.html#tymethod.is_terminal\" class=\"fn\">is_terminal</a>(&amp;self) -&gt; <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.bool.html\">bool</a></h4></section><details class=\"toggle method-toggle\" open><summary><section id=\"method.current_choice\" class=\"method\"><a class=\"src rightside\" href=\"src/anstream/auto.rs.html#175-182\">source</a><h4 class=\"code-header\">pub fn <a href=\"anstream/struct.AutoStream.html#tymethod.current_choice\" class=\"fn\">current_choice</a>(&amp;self) -&gt; <a class=\"enum\" href=\"anstream/enum.ColorChoice.html\" title=\"enum anstream::ColorChoice\">ColorChoice</a></h4></section></summary><div class=\"docblock\"><p>Prefer <a href=\"anstream/struct.AutoStream.html#method.choice\" title=\"associated function anstream::AutoStream::choice\"><code>AutoStream::choice</code></a></p>\n<p>This doesn’t report what is requested but what is currently active.</p>\n</div></details></div></details>",0,"anstream::Stdout","anstream::Stderr"],["<details class=\"toggle implementors-toggle\" open><summary><section id=\"impl-AutoStream%3CStderr%3E\" class=\"impl\"><a class=\"src rightside\" href=\"src/anstream/auto.rs.html#231-247\">source</a><a href=\"#impl-AutoStream%3CStderr%3E\" class=\"anchor\">§</a><h3 class=\"code-header\">impl <a class=\"struct\" href=\"anstream/struct.AutoStream.html\" title=\"struct anstream::AutoStream\">AutoStream</a>&lt;<a class=\"struct\" href=\"https://doc.rust-lang.org/1.78.0/std/io/stdio/struct.Stderr.html\" title=\"struct std::io::stdio::Stderr\">Stderr</a>&gt;</h3></section></summary><div class=\"impl-items\"><details class=\"toggle method-toggle\" open><summary><section id=\"method.lock\" class=\"method\"><a class=\"src rightside\" href=\"src/anstream/auto.rs.html#238-246\">source</a><h4 class=\"code-header\">pub fn <a href=\"anstream/struct.AutoStream.html#tymethod.lock\" class=\"fn\">lock</a>(self) -&gt; <a class=\"struct\" href=\"anstream/struct.AutoStream.html\" title=\"struct anstream::AutoStream\">AutoStream</a>&lt;<a class=\"struct\" href=\"https://doc.rust-lang.org/1.78.0/std/io/stdio/struct.StderrLock.html\" title=\"struct std::io::stdio::StderrLock\">StderrLock</a>&lt;'static&gt;&gt; <a href=\"#\" class=\"tooltip\" data-notable-ty=\"AutoStream&lt;StderrLock&lt;&#39;static&gt;&gt;\">ⓘ</a></h4></section></summary><div class=\"docblock\"><p>Get exclusive access to the <code>AutoStream</code></p>\n<p>Why?</p>\n<ul>\n<li>Faster performance when writing in a loop</li>\n<li>Avoid other threads interleaving output with the current thread</li>\n</ul>\n</div></details></div></details>",0,"anstream::Stderr"],["<details class=\"toggle implementors-toggle\" open><summary><section id=\"impl-AutoStream%3CStdout%3E\" class=\"impl\"><a class=\"src rightside\" href=\"src/anstream/auto.rs.html#213-229\">source</a><a href=\"#impl-AutoStream%3CStdout%3E\" class=\"anchor\">§</a><h3 class=\"code-header\">impl <a class=\"struct\" href=\"anstream/struct.AutoStream.html\" title=\"struct anstream::AutoStream\">AutoStream</a>&lt;<a class=\"struct\" href=\"https://doc.rust-lang.org/1.78.0/std/io/stdio/struct.Stdout.html\" title=\"struct std::io::stdio::Stdout\">Stdout</a>&gt;</h3></section></summary><div class=\"impl-items\"><details class=\"toggle method-toggle\" open><summary><section id=\"method.lock\" class=\"method\"><a class=\"src rightside\" href=\"src/anstream/auto.rs.html#220-228\">source</a><h4 class=\"code-header\">pub fn <a href=\"anstream/struct.AutoStream.html#tymethod.lock\" class=\"fn\">lock</a>(self) -&gt; <a class=\"struct\" href=\"anstream/struct.AutoStream.html\" title=\"struct anstream::AutoStream\">AutoStream</a>&lt;<a class=\"struct\" href=\"https://doc.rust-lang.org/1.78.0/std/io/stdio/struct.StdoutLock.html\" title=\"struct std::io::stdio::StdoutLock\">StdoutLock</a>&lt;'static&gt;&gt; <a href=\"#\" class=\"tooltip\" data-notable-ty=\"AutoStream&lt;StdoutLock&lt;&#39;static&gt;&gt;\">ⓘ</a></h4></section></summary><div class=\"docblock\"><p>Get exclusive access to the <code>AutoStream</code></p>\n<p>Why?</p>\n<ul>\n<li>Faster performance when writing in a loop</li>\n<li>Avoid other threads interleaving output with the current thread</li>\n</ul>\n</div></details></div></details>",0,"anstream::Stdout"],["<details class=\"toggle implementors-toggle\" open><summary><section id=\"impl-Debug-for-AutoStream%3CS%3E\" class=\"impl\"><a class=\"src rightside\" href=\"src/anstream/auto.rs.html#18\">source</a><a href=\"#impl-Debug-for-AutoStream%3CS%3E\" class=\"anchor\">§</a><h3 class=\"code-header\">impl&lt;S: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.78.0/core/fmt/trait.Debug.html\" title=\"trait core::fmt::Debug\">Debug</a> + <a class=\"trait\" href=\"anstream/stream/trait.RawStream.html\" title=\"trait anstream::stream::RawStream\">RawStream</a>&gt; <a class=\"trait\" href=\"https://doc.rust-lang.org/1.78.0/core/fmt/trait.Debug.html\" title=\"trait core::fmt::Debug\">Debug</a> for <a class=\"struct\" href=\"anstream/struct.AutoStream.html\" title=\"struct anstream::AutoStream\">AutoStream</a>&lt;S&gt;</h3></section></summary><div class=\"impl-items\"><details class=\"toggle method-toggle\" open><summary><section id=\"method.fmt\" class=\"method trait-impl\"><a class=\"src rightside\" href=\"src/anstream/auto.rs.html#18\">source</a><a href=\"#method.fmt\" class=\"anchor\">§</a><h4 class=\"code-header\">fn <a href=\"https://doc.rust-lang.org/1.78.0/core/fmt/trait.Debug.html#tymethod.fmt\" class=\"fn\">fmt</a>(&amp;self, f: &amp;mut <a class=\"struct\" href=\"https://doc.rust-lang.org/1.78.0/core/fmt/struct.Formatter.html\" title=\"struct core::fmt::Formatter\">Formatter</a>&lt;'_&gt;) -&gt; <a class=\"type\" href=\"https://doc.rust-lang.org/1.78.0/core/fmt/type.Result.html\" title=\"type core::fmt::Result\">Result</a></h4></section></summary><div class='docblock'>Formats the value using the given formatter. <a href=\"https://doc.rust-lang.org/1.78.0/core/fmt/trait.Debug.html#tymethod.fmt\">Read more</a></div></details></div></details>","Debug","anstream::Stdout","anstream::Stderr"],["<details class=\"toggle implementors-toggle\" open><summary><section id=\"impl-Write-for-AutoStream%3CS%3E\" class=\"impl\"><a class=\"src rightside\" href=\"src/anstream/auto.rs.html#249-301\">source</a><a href=\"#impl-Write-for-AutoStream%3CS%3E\" class=\"anchor\">§</a><h3 class=\"code-header\">impl&lt;S&gt; <a class=\"trait\" href=\"https://doc.rust-lang.org/1.78.0/std/io/trait.Write.html\" title=\"trait std::io::Write\">Write</a> for <a class=\"struct\" href=\"anstream/struct.AutoStream.html\" title=\"struct anstream::AutoStream\">AutoStream</a>&lt;S&gt;<div class=\"where\">where\n    S: <a class=\"trait\" href=\"anstream/stream/trait.RawStream.html\" title=\"trait anstream::stream::RawStream\">RawStream</a> + <a class=\"trait\" href=\"anstream/stream/trait.AsLockedWrite.html\" title=\"trait anstream::stream::AsLockedWrite\">AsLockedWrite</a>,</div></h3></section></summary><div class=\"impl-items\"><details class=\"toggle method-toggle\" open><summary><section id=\"method.write\" class=\"method trait-impl\"><a class=\"src rightside\" href=\"src/anstream/auto.rs.html#255-262\">source</a><a href=\"#method.write\" class=\"anchor\">§</a><h4 class=\"code-header\">fn <a href=\"https://doc.rust-lang.org/1.78.0/std/io/trait.Write.html#tymethod.write\" class=\"fn\">write</a>(&amp;mut self, buf: &amp;[<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.u8.html\">u8</a>]) -&gt; <a class=\"type\" href=\"https://doc.rust-lang.org/1.78.0/std/io/error/type.Result.html\" title=\"type std::io::error::Result\">Result</a>&lt;<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.usize.html\">usize</a>&gt;</h4></section></summary><div class='docblock'>Write a buffer into this writer, returning how many bytes were written. <a href=\"https://doc.rust-lang.org/1.78.0/std/io/trait.Write.html#tymethod.write\">Read more</a></div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.write_vectored\" class=\"method trait-impl\"><a class=\"src rightside\" href=\"src/anstream/auto.rs.html#264-271\">source</a><a href=\"#method.write_vectored\" class=\"anchor\">§</a><h4 class=\"code-header\">fn <a href=\"https://doc.rust-lang.org/1.78.0/std/io/trait.Write.html#method.write_vectored\" class=\"fn\">write_vectored</a>(&amp;mut self, bufs: &amp;[<a class=\"struct\" href=\"https://doc.rust-lang.org/1.78.0/std/io/struct.IoSlice.html\" title=\"struct std::io::IoSlice\">IoSlice</a>&lt;'_&gt;]) -&gt; <a class=\"type\" href=\"https://doc.rust-lang.org/1.78.0/std/io/error/type.Result.html\" title=\"type std::io::error::Result\">Result</a>&lt;<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.usize.html\">usize</a>&gt;</h4></section></summary><div class='docblock'>Like <a href=\"https://doc.rust-lang.org/1.78.0/std/io/trait.Write.html#tymethod.write\" title=\"method std::io::Write::write\"><code>write</code></a>, except that it writes from a slice of buffers. <a href=\"https://doc.rust-lang.org/1.78.0/std/io/trait.Write.html#method.write_vectored\">Read more</a></div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.flush\" class=\"method trait-impl\"><a class=\"src rightside\" href=\"src/anstream/auto.rs.html#274-281\">source</a><a href=\"#method.flush\" class=\"anchor\">§</a><h4 class=\"code-header\">fn <a href=\"https://doc.rust-lang.org/1.78.0/std/io/trait.Write.html#tymethod.flush\" class=\"fn\">flush</a>(&amp;mut self) -&gt; <a class=\"type\" href=\"https://doc.rust-lang.org/1.78.0/std/io/error/type.Result.html\" title=\"type std::io::error::Result\">Result</a>&lt;<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.unit.html\">()</a>&gt;</h4></section></summary><div class='docblock'>Flush this output stream, ensuring that all intermediately buffered\ncontents reach their destination. <a href=\"https://doc.rust-lang.org/1.78.0/std/io/trait.Write.html#tymethod.flush\">Read more</a></div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.write_all\" class=\"method trait-impl\"><a class=\"src rightside\" href=\"src/anstream/auto.rs.html#283-290\">source</a><a href=\"#method.write_all\" class=\"anchor\">§</a><h4 class=\"code-header\">fn <a href=\"https://doc.rust-lang.org/1.78.0/std/io/trait.Write.html#method.write_all\" class=\"fn\">write_all</a>(&amp;mut self, buf: &amp;[<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.u8.html\">u8</a>]) -&gt; <a class=\"type\" href=\"https://doc.rust-lang.org/1.78.0/std/io/error/type.Result.html\" title=\"type std::io::error::Result\">Result</a>&lt;<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.unit.html\">()</a>&gt;</h4></section></summary><div class='docblock'>Attempts to write an entire buffer into this writer. <a href=\"https://doc.rust-lang.org/1.78.0/std/io/trait.Write.html#method.write_all\">Read more</a></div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.write_fmt\" class=\"method trait-impl\"><a class=\"src rightside\" href=\"src/anstream/auto.rs.html#293-300\">source</a><a href=\"#method.write_fmt\" class=\"anchor\">§</a><h4 class=\"code-header\">fn <a href=\"https://doc.rust-lang.org/1.78.0/std/io/trait.Write.html#method.write_fmt\" class=\"fn\">write_fmt</a>(&amp;mut self, args: <a class=\"struct\" href=\"https://doc.rust-lang.org/1.78.0/core/fmt/struct.Arguments.html\" title=\"struct core::fmt::Arguments\">Arguments</a>&lt;'_&gt;) -&gt; <a class=\"type\" href=\"https://doc.rust-lang.org/1.78.0/std/io/error/type.Result.html\" title=\"type std::io::error::Result\">Result</a>&lt;<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.unit.html\">()</a>&gt;</h4></section></summary><div class='docblock'>Writes a formatted string into this writer, returning any error\nencountered. <a href=\"https://doc.rust-lang.org/1.78.0/std/io/trait.Write.html#method.write_fmt\">Read more</a></div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.is_write_vectored\" class=\"method trait-impl\"><a class=\"src rightside\" href=\"https://doc.rust-lang.org/1.78.0/src/std/io/mod.rs.html#1639\">source</a><a href=\"#method.is_write_vectored\" class=\"anchor\">§</a><h4 class=\"code-header\">fn <a href=\"https://doc.rust-lang.org/1.78.0/std/io/trait.Write.html#method.is_write_vectored\" class=\"fn\">is_write_vectored</a>(&amp;self) -&gt; <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.bool.html\">bool</a></h4></section></summary><span class=\"item-info\"><div class=\"stab unstable\"><span class=\"emoji\">🔬</span><span>This is a nightly-only experimental API. (<code>can_vector</code>)</span></div></span><div class='docblock'>Determines if this <code>Write</code>r has an efficient <a href=\"https://doc.rust-lang.org/1.78.0/std/io/trait.Write.html#method.write_vectored\" title=\"method std::io::Write::write_vectored\"><code>write_vectored</code></a>\nimplementation. <a href=\"https://doc.rust-lang.org/1.78.0/std/io/trait.Write.html#method.is_write_vectored\">Read more</a></div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.write_all_vectored\" class=\"method trait-impl\"><a class=\"src rightside\" href=\"https://doc.rust-lang.org/1.78.0/src/std/io/mod.rs.html#1766\">source</a><a href=\"#method.write_all_vectored\" class=\"anchor\">§</a><h4 class=\"code-header\">fn <a href=\"https://doc.rust-lang.org/1.78.0/std/io/trait.Write.html#method.write_all_vectored\" class=\"fn\">write_all_vectored</a>(&amp;mut self, bufs: &amp;mut [<a class=\"struct\" href=\"https://doc.rust-lang.org/1.78.0/std/io/struct.IoSlice.html\" title=\"struct std::io::IoSlice\">IoSlice</a>&lt;'_&gt;]) -&gt; <a class=\"enum\" href=\"https://doc.rust-lang.org/1.78.0/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.unit.html\">()</a>, <a class=\"struct\" href=\"https://doc.rust-lang.org/1.78.0/std/io/error/struct.Error.html\" title=\"struct std::io::error::Error\">Error</a>&gt;</h4></section></summary><span class=\"item-info\"><div class=\"stab unstable\"><span class=\"emoji\">🔬</span><span>This is a nightly-only experimental API. (<code>write_all_vectored</code>)</span></div></span><div class='docblock'>Attempts to write multiple buffers into this writer. <a href=\"https://doc.rust-lang.org/1.78.0/std/io/trait.Write.html#method.write_all_vectored\">Read more</a></div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.by_ref\" class=\"method trait-impl\"><span class=\"rightside\"><span class=\"since\" title=\"Stable since Rust version 1.0.0\">1.0.0</span> · <a class=\"src\" href=\"https://doc.rust-lang.org/1.78.0/src/std/io/mod.rs.html#1878-1880\">source</a></span><a href=\"#method.by_ref\" class=\"anchor\">§</a><h4 class=\"code-header\">fn <a href=\"https://doc.rust-lang.org/1.78.0/std/io/trait.Write.html#method.by_ref\" class=\"fn\">by_ref</a>(&amp;mut self) -&gt; <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.reference.html\">&amp;mut Self</a><div class=\"where\">where\n    Self: <a class=\"trait\" href=\"https://doc.rust-lang.org/1.78.0/core/marker/trait.Sized.html\" title=\"trait core::marker::Sized\">Sized</a>,</div></h4></section></summary><div class='docblock'>Creates a “by reference” adapter for this instance of <code>Write</code>. <a href=\"https://doc.rust-lang.org/1.78.0/std/io/trait.Write.html#method.by_ref\">Read more</a></div></details></div></details>","Write","anstream::Stdout","anstream::Stderr"]]
};if (window.register_type_impls) {window.register_type_impls(type_impls);} else {window.pending_type_impls = type_impls;}})()