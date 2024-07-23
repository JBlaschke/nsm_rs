(function() {var type_impls = {
"pnet":[["<details class=\"toggle implementors-toggle\" open><summary><section id=\"impl-Debug-for-MutableExtensionPacket%3C'p%3E\" class=\"impl\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><a href=\"#impl-Debug-for-MutableExtensionPacket%3C'p%3E\" class=\"anchor\">§</a><h3 class=\"code-header\">impl&lt;'p&gt; <a class=\"trait\" href=\"https://doc.rust-lang.org/1.78.0/core/fmt/trait.Debug.html\" title=\"trait core::fmt::Debug\">Debug</a> for <a class=\"struct\" href=\"pnet/packet/ipv6/struct.MutableExtensionPacket.html\" title=\"struct pnet::packet::ipv6::MutableExtensionPacket\">MutableExtensionPacket</a>&lt;'p&gt;</h3></section></summary><div class=\"impl-items\"><details class=\"toggle method-toggle\" open><summary><section id=\"method.fmt\" class=\"method trait-impl\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><a href=\"#method.fmt\" class=\"anchor\">§</a><h4 class=\"code-header\">fn <a href=\"https://doc.rust-lang.org/1.78.0/core/fmt/trait.Debug.html#tymethod.fmt\" class=\"fn\">fmt</a>(&amp;self, fmt: &amp;mut <a class=\"struct\" href=\"https://doc.rust-lang.org/1.78.0/core/fmt/struct.Formatter.html\" title=\"struct core::fmt::Formatter\">Formatter</a>&lt;'_&gt;) -&gt; <a class=\"enum\" href=\"https://doc.rust-lang.org/1.78.0/core/result/enum.Result.html\" title=\"enum core::result::Result\">Result</a>&lt;<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.unit.html\">()</a>, <a class=\"struct\" href=\"https://doc.rust-lang.org/1.78.0/core/fmt/struct.Error.html\" title=\"struct core::fmt::Error\">Error</a>&gt;</h4></section></summary><div class='docblock'>Formats the value using the given formatter. <a href=\"https://doc.rust-lang.org/1.78.0/core/fmt/trait.Debug.html#tymethod.fmt\">Read more</a></div></details></div></details>","Debug","pnet::packet::ipv6::MutableHopByHopPacket","pnet::packet::ipv6::MutableDestinationPacket"],["<details class=\"toggle implementors-toggle\" open><summary><section id=\"impl-FromPacket-for-MutableExtensionPacket%3C'p%3E\" class=\"impl\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><a href=\"#impl-FromPacket-for-MutableExtensionPacket%3C'p%3E\" class=\"anchor\">§</a><h3 class=\"code-header\">impl&lt;'p&gt; <a class=\"trait\" href=\"pnet/packet/trait.FromPacket.html\" title=\"trait pnet::packet::FromPacket\">FromPacket</a> for <a class=\"struct\" href=\"pnet/packet/ipv6/struct.MutableExtensionPacket.html\" title=\"struct pnet::packet::ipv6::MutableExtensionPacket\">MutableExtensionPacket</a>&lt;'p&gt;</h3></section></summary><div class=\"impl-items\"><details class=\"toggle\" open><summary><section id=\"associatedtype.T\" class=\"associatedtype trait-impl\"><a href=\"#associatedtype.T\" class=\"anchor\">§</a><h4 class=\"code-header\">type <a href=\"pnet/packet/trait.FromPacket.html#associatedtype.T\" class=\"associatedtype\">T</a> = <a class=\"struct\" href=\"pnet/packet/ipv6/struct.Extension.html\" title=\"struct pnet::packet::ipv6::Extension\">Extension</a></h4></section></summary><div class='docblock'>The type of the packet to convert from.</div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.from_packet\" class=\"method trait-impl\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><a href=\"#method.from_packet\" class=\"anchor\">§</a><h4 class=\"code-header\">fn <a href=\"pnet/packet/trait.FromPacket.html#tymethod.from_packet\" class=\"fn\">from_packet</a>(&amp;self) -&gt; <a class=\"struct\" href=\"pnet/packet/ipv6/struct.Extension.html\" title=\"struct pnet::packet::ipv6::Extension\">Extension</a></h4></section></summary><div class='docblock'>Converts a wire-format packet to #[packet] struct format.</div></details></div></details>","FromPacket","pnet::packet::ipv6::MutableHopByHopPacket","pnet::packet::ipv6::MutableDestinationPacket"],["<details class=\"toggle implementors-toggle\" open><summary><section id=\"impl-MutableExtensionPacket%3C'a%3E\" class=\"impl\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><a href=\"#impl-MutableExtensionPacket%3C'a%3E\" class=\"anchor\">§</a><h3 class=\"code-header\">impl&lt;'a&gt; <a class=\"struct\" href=\"pnet/packet/ipv6/struct.MutableExtensionPacket.html\" title=\"struct pnet::packet::ipv6::MutableExtensionPacket\">MutableExtensionPacket</a>&lt;'a&gt;</h3></section></summary><div class=\"impl-items\"><details class=\"toggle method-toggle\" open><summary><section id=\"method.new\" class=\"method\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><h4 class=\"code-header\">pub fn <a href=\"pnet/packet/ipv6/struct.MutableExtensionPacket.html#tymethod.new\" class=\"fn\">new</a>&lt;'p&gt;(packet: &amp;'p mut [<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.u8.html\">u8</a>]) -&gt; <a class=\"enum\" href=\"https://doc.rust-lang.org/1.78.0/core/option/enum.Option.html\" title=\"enum core::option::Option\">Option</a>&lt;<a class=\"struct\" href=\"pnet/packet/ipv6/struct.MutableExtensionPacket.html\" title=\"struct pnet::packet::ipv6::MutableExtensionPacket\">MutableExtensionPacket</a>&lt;'p&gt;&gt;</h4></section></summary><div class=\"docblock\"><p>Constructs a new MutableExtensionPacket. If the provided buffer is less than the minimum required\npacket size, this will return None.</p>\n</div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.owned\" class=\"method\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><h4 class=\"code-header\">pub fn <a href=\"pnet/packet/ipv6/struct.MutableExtensionPacket.html#tymethod.owned\" class=\"fn\">owned</a>(packet: <a class=\"struct\" href=\"https://doc.rust-lang.org/1.78.0/alloc/vec/struct.Vec.html\" title=\"struct alloc::vec::Vec\">Vec</a>&lt;<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.u8.html\">u8</a>&gt;) -&gt; <a class=\"enum\" href=\"https://doc.rust-lang.org/1.78.0/core/option/enum.Option.html\" title=\"enum core::option::Option\">Option</a>&lt;<a class=\"struct\" href=\"pnet/packet/ipv6/struct.MutableExtensionPacket.html\" title=\"struct pnet::packet::ipv6::MutableExtensionPacket\">MutableExtensionPacket</a>&lt;'static&gt;&gt;</h4></section></summary><div class=\"docblock\"><p>Constructs a new MutableExtensionPacket. If the provided buffer is less than the minimum required\npacket size, this will return None. With this constructor the MutableExtensionPacket will\nown its own data and the underlying buffer will be dropped when the MutableExtensionPacket is.</p>\n</div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.to_immutable\" class=\"method\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><h4 class=\"code-header\">pub fn <a href=\"pnet/packet/ipv6/struct.MutableExtensionPacket.html#tymethod.to_immutable\" class=\"fn\">to_immutable</a>&lt;'p&gt;(&amp;'p self) -&gt; <a class=\"struct\" href=\"pnet/packet/ipv6/struct.ExtensionPacket.html\" title=\"struct pnet::packet::ipv6::ExtensionPacket\">ExtensionPacket</a>&lt;'p&gt;</h4></section></summary><div class=\"docblock\"><p>Maps from a MutableExtensionPacket to a ExtensionPacket</p>\n</div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.consume_to_immutable\" class=\"method\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><h4 class=\"code-header\">pub fn <a href=\"pnet/packet/ipv6/struct.MutableExtensionPacket.html#tymethod.consume_to_immutable\" class=\"fn\">consume_to_immutable</a>(self) -&gt; <a class=\"struct\" href=\"pnet/packet/ipv6/struct.ExtensionPacket.html\" title=\"struct pnet::packet::ipv6::ExtensionPacket\">ExtensionPacket</a>&lt;'a&gt;</h4></section></summary><div class=\"docblock\"><p>Maps from a MutableExtensionPacket to a ExtensionPacket while consuming the source</p>\n</div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.minimum_packet_size\" class=\"method\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><h4 class=\"code-header\">pub const fn <a href=\"pnet/packet/ipv6/struct.MutableExtensionPacket.html#tymethod.minimum_packet_size\" class=\"fn\">minimum_packet_size</a>() -&gt; <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.usize.html\">usize</a></h4></section></summary><div class=\"docblock\"><p>The minimum size (in bytes) a packet of this type can be. It’s based on the total size\nof the fixed-size fields.</p>\n</div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.packet_size\" class=\"method\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><h4 class=\"code-header\">pub fn <a href=\"pnet/packet/ipv6/struct.MutableExtensionPacket.html#tymethod.packet_size\" class=\"fn\">packet_size</a>(_packet: &amp;<a class=\"struct\" href=\"pnet/packet/ipv6/struct.Extension.html\" title=\"struct pnet::packet::ipv6::Extension\">Extension</a>) -&gt; <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.usize.html\">usize</a></h4></section></summary><div class=\"docblock\"><p>The size (in bytes) of a Extension instance when converted into\na byte-array</p>\n</div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.populate\" class=\"method\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><h4 class=\"code-header\">pub fn <a href=\"pnet/packet/ipv6/struct.MutableExtensionPacket.html#tymethod.populate\" class=\"fn\">populate</a>(&amp;mut self, packet: &amp;<a class=\"struct\" href=\"pnet/packet/ipv6/struct.Extension.html\" title=\"struct pnet::packet::ipv6::Extension\">Extension</a>)</h4></section></summary><div class=\"docblock\"><p>Populates a ExtensionPacket using a Extension structure</p>\n</div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.get_next_header\" class=\"method\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><h4 class=\"code-header\">pub fn <a href=\"pnet/packet/ipv6/struct.MutableExtensionPacket.html#tymethod.get_next_header\" class=\"fn\">get_next_header</a>(&amp;self) -&gt; <a class=\"struct\" href=\"pnet/packet/ip/struct.IpNextHeaderProtocol.html\" title=\"struct pnet::packet::ip::IpNextHeaderProtocol\">IpNextHeaderProtocol</a></h4></section></summary><div class=\"docblock\"><p>Get the value of the next_header field</p>\n</div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.get_hdr_ext_len\" class=\"method\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><h4 class=\"code-header\">pub fn <a href=\"pnet/packet/ipv6/struct.MutableExtensionPacket.html#tymethod.get_hdr_ext_len\" class=\"fn\">get_hdr_ext_len</a>(&amp;self) -&gt; <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.u8.html\">u8</a></h4></section></summary><div class=\"docblock\"><p>Get the hdr_ext_len field.</p>\n</div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.set_next_header\" class=\"method\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><h4 class=\"code-header\">pub fn <a href=\"pnet/packet/ipv6/struct.MutableExtensionPacket.html#tymethod.set_next_header\" class=\"fn\">set_next_header</a>(&amp;mut self, val: <a class=\"struct\" href=\"pnet/packet/ip/struct.IpNextHeaderProtocol.html\" title=\"struct pnet::packet::ip::IpNextHeaderProtocol\">IpNextHeaderProtocol</a>)</h4></section></summary><div class=\"docblock\"><p>Set the value of the next_header field.</p>\n</div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.set_hdr_ext_len\" class=\"method\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><h4 class=\"code-header\">pub fn <a href=\"pnet/packet/ipv6/struct.MutableExtensionPacket.html#tymethod.set_hdr_ext_len\" class=\"fn\">set_hdr_ext_len</a>(&amp;mut self, val: <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.u8.html\">u8</a>)</h4></section></summary><div class=\"docblock\"><p>Set the hdr_ext_len field.</p>\n</div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.set_options\" class=\"method\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><h4 class=\"code-header\">pub fn <a href=\"pnet/packet/ipv6/struct.MutableExtensionPacket.html#tymethod.set_options\" class=\"fn\">set_options</a>(&amp;mut self, vals: &amp;[<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.u8.html\">u8</a>])</h4></section></summary><div class=\"docblock\"><p>Set the value of the options field (copies contents)</p>\n</div></details></div></details>",0,"pnet::packet::ipv6::MutableHopByHopPacket","pnet::packet::ipv6::MutableDestinationPacket"],["<details class=\"toggle implementors-toggle\" open><summary><section id=\"impl-MutablePacket-for-MutableExtensionPacket%3C'a%3E\" class=\"impl\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><a href=\"#impl-MutablePacket-for-MutableExtensionPacket%3C'a%3E\" class=\"anchor\">§</a><h3 class=\"code-header\">impl&lt;'a&gt; <a class=\"trait\" href=\"pnet/packet/trait.MutablePacket.html\" title=\"trait pnet::packet::MutablePacket\">MutablePacket</a> for <a class=\"struct\" href=\"pnet/packet/ipv6/struct.MutableExtensionPacket.html\" title=\"struct pnet::packet::ipv6::MutableExtensionPacket\">MutableExtensionPacket</a>&lt;'a&gt;</h3></section></summary><div class=\"impl-items\"><details class=\"toggle method-toggle\" open><summary><section id=\"method.packet_mut\" class=\"method trait-impl\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><a href=\"#method.packet_mut\" class=\"anchor\">§</a><h4 class=\"code-header\">fn <a href=\"pnet/packet/trait.MutablePacket.html#tymethod.packet_mut\" class=\"fn\">packet_mut</a>&lt;'p&gt;(&amp;'p mut self) -&gt; &amp;'p mut [<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.u8.html\">u8</a>] <a href=\"#\" class=\"tooltip\" data-notable-ty=\"&amp;&#39;p mut [u8]\">ⓘ</a></h4></section></summary><div class='docblock'>Retreive the underlying, mutable, buffer for the packet.</div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.payload_mut\" class=\"method trait-impl\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><a href=\"#method.payload_mut\" class=\"anchor\">§</a><h4 class=\"code-header\">fn <a href=\"pnet/packet/trait.MutablePacket.html#tymethod.payload_mut\" class=\"fn\">payload_mut</a>&lt;'p&gt;(&amp;'p mut self) -&gt; &amp;'p mut [<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.u8.html\">u8</a>] <a href=\"#\" class=\"tooltip\" data-notable-ty=\"&amp;&#39;p mut [u8]\">ⓘ</a></h4></section></summary><div class='docblock'>Retreive the mutable payload for the packet.</div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.clone_from\" class=\"method trait-impl\"><a class=\"src rightside\" href=\"src/pnet_macros_support/packet.rs.html#35\">source</a><a href=\"#method.clone_from\" class=\"anchor\">§</a><h4 class=\"code-header\">fn <a href=\"pnet/packet/trait.MutablePacket.html#method.clone_from\" class=\"fn\">clone_from</a>&lt;T&gt;(&amp;mut self, other: <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.reference.html\">&amp;T</a>)<div class=\"where\">where\n    T: <a class=\"trait\" href=\"pnet/packet/trait.Packet.html\" title=\"trait pnet::packet::Packet\">Packet</a>,</div></h4></section></summary><div class='docblock'>Initialize this packet by cloning another.</div></details></div></details>","MutablePacket","pnet::packet::ipv6::MutableHopByHopPacket","pnet::packet::ipv6::MutableDestinationPacket"],["<details class=\"toggle implementors-toggle\" open><summary><section id=\"impl-Packet-for-MutableExtensionPacket%3C'a%3E\" class=\"impl\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><a href=\"#impl-Packet-for-MutableExtensionPacket%3C'a%3E\" class=\"anchor\">§</a><h3 class=\"code-header\">impl&lt;'a&gt; <a class=\"trait\" href=\"pnet/packet/trait.Packet.html\" title=\"trait pnet::packet::Packet\">Packet</a> for <a class=\"struct\" href=\"pnet/packet/ipv6/struct.MutableExtensionPacket.html\" title=\"struct pnet::packet::ipv6::MutableExtensionPacket\">MutableExtensionPacket</a>&lt;'a&gt;</h3></section></summary><div class=\"impl-items\"><details class=\"toggle method-toggle\" open><summary><section id=\"method.packet\" class=\"method trait-impl\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><a href=\"#method.packet\" class=\"anchor\">§</a><h4 class=\"code-header\">fn <a href=\"pnet/packet/trait.Packet.html#tymethod.packet\" class=\"fn\">packet</a>&lt;'p&gt;(&amp;'p self) -&gt; &amp;'p [<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.u8.html\">u8</a>] <a href=\"#\" class=\"tooltip\" data-notable-ty=\"&amp;&#39;p [u8]\">ⓘ</a></h4></section></summary><div class='docblock'>Retrieve the underlying buffer for the packet.</div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.payload\" class=\"method trait-impl\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><a href=\"#method.payload\" class=\"anchor\">§</a><h4 class=\"code-header\">fn <a href=\"pnet/packet/trait.Packet.html#tymethod.payload\" class=\"fn\">payload</a>&lt;'p&gt;(&amp;'p self) -&gt; &amp;'p [<a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.u8.html\">u8</a>] <a href=\"#\" class=\"tooltip\" data-notable-ty=\"&amp;&#39;p [u8]\">ⓘ</a></h4></section></summary><div class='docblock'>Retrieve the payload for the packet.</div></details></div></details>","Packet","pnet::packet::ipv6::MutableHopByHopPacket","pnet::packet::ipv6::MutableDestinationPacket"],["<details class=\"toggle implementors-toggle\" open><summary><section id=\"impl-PacketSize-for-MutableExtensionPacket%3C'a%3E\" class=\"impl\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><a href=\"#impl-PacketSize-for-MutableExtensionPacket%3C'a%3E\" class=\"anchor\">§</a><h3 class=\"code-header\">impl&lt;'a&gt; <a class=\"trait\" href=\"pnet/packet/trait.PacketSize.html\" title=\"trait pnet::packet::PacketSize\">PacketSize</a> for <a class=\"struct\" href=\"pnet/packet/ipv6/struct.MutableExtensionPacket.html\" title=\"struct pnet::packet::ipv6::MutableExtensionPacket\">MutableExtensionPacket</a>&lt;'a&gt;</h3></section></summary><div class=\"impl-items\"><details class=\"toggle method-toggle\" open><summary><section id=\"method.packet_size\" class=\"method trait-impl\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><a href=\"#method.packet_size\" class=\"anchor\">§</a><h4 class=\"code-header\">fn <a href=\"pnet/packet/trait.PacketSize.html#tymethod.packet_size\" class=\"fn\">packet_size</a>(&amp;self) -&gt; <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.usize.html\">usize</a></h4></section></summary><div class='docblock'>Get the calculated size of the packet.</div></details></div></details>","PacketSize","pnet::packet::ipv6::MutableHopByHopPacket","pnet::packet::ipv6::MutableDestinationPacket"],["<details class=\"toggle implementors-toggle\" open><summary><section id=\"impl-PartialEq-for-MutableExtensionPacket%3C'p%3E\" class=\"impl\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><a href=\"#impl-PartialEq-for-MutableExtensionPacket%3C'p%3E\" class=\"anchor\">§</a><h3 class=\"code-header\">impl&lt;'p&gt; <a class=\"trait\" href=\"https://doc.rust-lang.org/1.78.0/core/cmp/trait.PartialEq.html\" title=\"trait core::cmp::PartialEq\">PartialEq</a> for <a class=\"struct\" href=\"pnet/packet/ipv6/struct.MutableExtensionPacket.html\" title=\"struct pnet::packet::ipv6::MutableExtensionPacket\">MutableExtensionPacket</a>&lt;'p&gt;</h3></section></summary><div class=\"impl-items\"><details class=\"toggle method-toggle\" open><summary><section id=\"method.eq\" class=\"method trait-impl\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><a href=\"#method.eq\" class=\"anchor\">§</a><h4 class=\"code-header\">fn <a href=\"https://doc.rust-lang.org/1.78.0/core/cmp/trait.PartialEq.html#tymethod.eq\" class=\"fn\">eq</a>(&amp;self, other: &amp;<a class=\"struct\" href=\"pnet/packet/ipv6/struct.MutableExtensionPacket.html\" title=\"struct pnet::packet::ipv6::MutableExtensionPacket\">MutableExtensionPacket</a>&lt;'p&gt;) -&gt; <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.bool.html\">bool</a></h4></section></summary><div class='docblock'>This method tests for <code>self</code> and <code>other</code> values to be equal, and is used\nby <code>==</code>.</div></details><details class=\"toggle method-toggle\" open><summary><section id=\"method.ne\" class=\"method trait-impl\"><span class=\"rightside\"><span class=\"since\" title=\"Stable since Rust version 1.0.0\">1.0.0</span> · <a class=\"src\" href=\"https://doc.rust-lang.org/1.78.0/src/core/cmp.rs.html#263\">source</a></span><a href=\"#method.ne\" class=\"anchor\">§</a><h4 class=\"code-header\">fn <a href=\"https://doc.rust-lang.org/1.78.0/core/cmp/trait.PartialEq.html#method.ne\" class=\"fn\">ne</a>(&amp;self, other: <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.reference.html\">&amp;Rhs</a>) -&gt; <a class=\"primitive\" href=\"https://doc.rust-lang.org/1.78.0/std/primitive.bool.html\">bool</a></h4></section></summary><div class='docblock'>This method tests for <code>!=</code>. The default implementation is almost always\nsufficient, and should not be overridden without very good reason.</div></details></div></details>","PartialEq","pnet::packet::ipv6::MutableHopByHopPacket","pnet::packet::ipv6::MutableDestinationPacket"],["<section id=\"impl-StructuralPartialEq-for-MutableExtensionPacket%3C'p%3E\" class=\"impl\"><a class=\"src rightside\" href=\"src/pnet_packet/ipv6.rs.html#46\">source</a><a href=\"#impl-StructuralPartialEq-for-MutableExtensionPacket%3C'p%3E\" class=\"anchor\">§</a><h3 class=\"code-header\">impl&lt;'p&gt; <a class=\"trait\" href=\"https://doc.rust-lang.org/1.78.0/core/marker/trait.StructuralPartialEq.html\" title=\"trait core::marker::StructuralPartialEq\">StructuralPartialEq</a> for <a class=\"struct\" href=\"pnet/packet/ipv6/struct.MutableExtensionPacket.html\" title=\"struct pnet::packet::ipv6::MutableExtensionPacket\">MutableExtensionPacket</a>&lt;'p&gt;</h3></section>","StructuralPartialEq","pnet::packet::ipv6::MutableHopByHopPacket","pnet::packet::ipv6::MutableDestinationPacket"]]
};if (window.register_type_impls) {window.register_type_impls(type_impls);} else {window.pending_type_impls = type_impls;}})()